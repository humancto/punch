//! Remote index client for the skills marketplace.
//!
//! Manages a local Git clone of the index repository and provides search,
//! version resolution, and skill fetching capabilities.

use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::{debug, info, warn};

use punch_types::PunchResult;

use crate::registry::{IndexEntry, IndexMeta, index_path_for_name};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default index repository URL.
pub const DEFAULT_INDEX_URL: &str = "https://github.com/humancto/punch-index.git";

/// Default cache directory name under ~/.punch/
const INDEX_DIR_NAME: &str = "index";
const CACHE_DIR_NAME: &str = "cache";

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Client for interacting with the remote skills index.
pub struct IndexClient {
    /// URL of the Git index repository.
    index_url: String,
    /// Local path for the Git clone of the index.
    index_dir: PathBuf,
    /// Local path for cached skill tarballs.
    cache_dir: PathBuf,
}

impl IndexClient {
    /// Create a new index client.
    ///
    /// - `index_url`: URL of the Git index repository
    /// - `base_dir`: Base directory for local state (e.g., `~/.punch/`)
    pub fn new(index_url: &str, base_dir: &Path) -> Self {
        Self {
            index_url: index_url.to_string(),
            index_dir: base_dir.join(INDEX_DIR_NAME),
            cache_dir: base_dir.join(CACHE_DIR_NAME),
        }
    }

    /// Create a client with default settings.
    pub fn with_defaults(base_dir: &Path) -> Self {
        Self::new(DEFAULT_INDEX_URL, base_dir)
    }

    /// Sync the local index with the remote.
    ///
    /// Performs `git clone` on first run, `git pull` on subsequent runs.
    pub fn sync(&self) -> PunchResult<()> {
        std::fs::create_dir_all(&self.index_dir).map_err(|e| {
            punch_types::PunchError::Config(format!("failed to create index directory: {}", e))
        })?;

        if self.index_dir.join(".git").exists() {
            // Pull latest
            info!(path = %self.index_dir.display(), "pulling index updates");
            let output = Command::new("git")
                .args(["pull", "--ff-only", "--quiet"])
                .current_dir(&self.index_dir)
                .output()
                .map_err(|e| {
                    punch_types::PunchError::Config(format!("failed to run git pull: {}", e))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!(stderr = %stderr, "git pull failed, continuing with existing index");
            }
        } else {
            // Clone fresh
            info!(url = %self.index_url, path = %self.index_dir.display(), "cloning index");
            let output = Command::new("git")
                .args([
                    "clone",
                    "--depth",
                    "1",
                    "--quiet",
                    &self.index_url,
                    self.index_dir.to_str().unwrap_or("."),
                ])
                .output()
                .map_err(|e| {
                    punch_types::PunchError::Config(format!("failed to run git clone: {}", e))
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(punch_types::PunchError::Config(format!(
                    "failed to clone index: {}",
                    stderr
                )));
            }
        }

        Ok(())
    }

    /// Search the local index for skills matching a query.
    ///
    /// Searches name, description, category, and tags.
    pub fn search(
        &self,
        query: &str,
        category: Option<&str>,
        tags: Option<&[String]>,
    ) -> PunchResult<Vec<IndexMeta>> {
        let entries = self.read_all_entries()?;
        let query_lower = query.to_lowercase();

        let results: Vec<IndexMeta> = entries
            .into_iter()
            .filter(|meta| {
                // Filter by category if specified
                if let Some(cat) = category
                    && !meta.name.contains(cat)
                {
                    return false;
                }

                // Filter by tags if specified
                if let Some(search_tags) = tags {
                    let meta_str = serde_json::to_string(meta)
                        .unwrap_or_default()
                        .to_lowercase();
                    if !search_tags
                        .iter()
                        .any(|t| meta_str.contains(&t.to_lowercase()))
                    {
                        return false;
                    }
                }

                // Match query against name
                if query.is_empty() {
                    return true;
                }
                meta.name.to_lowercase().contains(&query_lower)
            })
            .collect();

        Ok(results)
    }

    /// Get a specific index entry by name and version.
    pub fn get_entry(&self, name: &str, version: &str) -> PunchResult<IndexEntry> {
        let meta = self.read_entry(name)?;
        meta.versions
            .into_iter()
            .find(|v| v.version == version)
            .ok_or_else(|| {
                punch_types::PunchError::Config(format!(
                    "version {} not found for skill '{}'",
                    version, name
                ))
            })
    }

    /// Resolve a version requirement to a concrete version.
    ///
    /// Currently supports exact versions only. Returns the latest version
    /// if `version_req` is "latest" or "*".
    pub fn resolve_version(&self, name: &str, version_req: &str) -> PunchResult<String> {
        let meta = self.read_entry(name)?;

        if meta.versions.is_empty() {
            return Err(punch_types::PunchError::Config(format!(
                "no versions found for skill '{}'",
                name
            )));
        }

        match version_req {
            "latest" | "*" | "" => Ok(meta.versions[0].version.clone()),
            exact => {
                if meta.versions.iter().any(|v| v.version == exact) {
                    Ok(exact.to_string())
                } else {
                    Err(punch_types::PunchError::Config(format!(
                        "version {} not found for skill '{}'. Available: {}",
                        exact,
                        name,
                        meta.versions
                            .iter()
                            .map(|v| v.version.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )))
                }
            }
        }
    }

    /// Fetch a skill tarball from the source URL.
    ///
    /// Uses the cache directory to avoid re-downloading.
    pub async fn fetch_skill(&self, entry: &IndexEntry) -> PunchResult<Vec<u8>> {
        // Check cache first
        let cache_key = format!("{}-{}.tar.gz", entry.name, entry.version);
        let cache_path = self.cache_dir.join(&cache_key);

        if cache_path.exists() {
            debug!(path = %cache_path.display(), "loading skill from cache");
            return std::fs::read(&cache_path).map_err(|e| {
                punch_types::PunchError::Config(format!("failed to read cached skill: {}", e))
            });
        }

        // Fetch from remote
        info!(url = %entry.source_url, "fetching skill tarball");
        let response = reqwest::get(&entry.source_url).await.map_err(|e| {
            punch_types::PunchError::Config(format!("failed to fetch skill: {}", e))
        })?;

        if !response.status().is_success() {
            return Err(punch_types::PunchError::Config(format!(
                "failed to fetch skill: HTTP {}",
                response.status()
            )));
        }

        let data = response.bytes().await.map_err(|e| {
            punch_types::PunchError::Config(format!("failed to read response: {}", e))
        })?;
        let data = data.to_vec();

        // Cache for next time
        std::fs::create_dir_all(&self.cache_dir).ok();
        if let Err(e) = std::fs::write(&cache_path, &data) {
            warn!(error = %e, "failed to cache skill tarball");
        }

        Ok(data)
    }

    /// Get the local index directory path.
    pub fn index_dir(&self) -> &Path {
        &self.index_dir
    }

    /// Get the local cache directory path.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    // -----------------------------------------------------------------------
    // Internal methods
    // -----------------------------------------------------------------------

    /// Read all index entries from the local clone.
    fn read_all_entries(&self) -> PunchResult<Vec<IndexMeta>> {
        let mut results = Vec::new();

        if !self.index_dir.exists() {
            return Ok(results);
        }

        // Walk the index directory looking for JSON files
        walk_index_dir(&self.index_dir, &mut results)?;
        Ok(results)
    }

    /// Read a single skill's metadata from the index.
    fn read_entry(&self, name: &str) -> PunchResult<IndexMeta> {
        let rel_path = index_path_for_name(name);
        let file_path = self.index_dir.join(&rel_path).with_extension("json");

        if !file_path.exists() {
            return Err(punch_types::PunchError::Config(format!(
                "skill '{}' not found in index (looked at {})",
                name,
                file_path.display()
            )));
        }

        let content = std::fs::read_to_string(&file_path)?;
        let meta: IndexMeta = serde_json::from_str(&content).map_err(|e| {
            punch_types::PunchError::Config(format!("invalid index entry for '{}': {}", name, e))
        })?;

        Ok(meta)
    }
}

/// Recursively walk a directory for index metadata JSON files.
fn walk_index_dir(dir: &Path, results: &mut Vec<IndexMeta>) -> PunchResult<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip .git directory
            if path.file_name().is_some_and(|n| n.to_str() == Some(".git")) {
                continue;
            }
            walk_index_dir(&path, results)?;
        } else if path.extension().is_some_and(|e| e == "json") {
            match std::fs::read_to_string(&path) {
                Ok(content) => match serde_json::from_str::<IndexMeta>(&content) {
                    Ok(meta) => results.push(meta),
                    Err(e) => {
                        debug!(path = %path.display(), error = %e, "skipping invalid index file");
                    }
                },
                Err(e) => {
                    debug!(path = %path.display(), error = %e, "failed to read index file");
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{IndexEntry, IndexMeta, ScanVerdict};

    #[test]
    fn test_client_creation() {
        let dir = tempfile::tempdir().unwrap();
        let client = IndexClient::new("https://example.com/index.git", dir.path());
        assert_eq!(client.index_url, "https://example.com/index.git");
        assert!(client.index_dir().ends_with("index"));
        assert!(client.cache_dir().ends_with("cache"));
    }

    #[test]
    fn test_client_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let client = IndexClient::with_defaults(dir.path());
        assert_eq!(client.index_url, DEFAULT_INDEX_URL);
    }

    #[test]
    fn test_read_all_entries_empty() {
        let dir = tempfile::tempdir().unwrap();
        let client = IndexClient::new("https://example.com/index.git", dir.path());
        let entries = client.read_all_entries().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_read_all_entries_with_data() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");

        // Create a skill entry in the index
        let skill_dir = index_dir.join("co");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let meta = IndexMeta {
            name: "code-reviewer".to_string(),
            versions: vec![IndexEntry {
                name: "code-reviewer".to_string(),
                version: "1.0.0".to_string(),
                checksum: "abc".to_string(),
                signature: "sig".to_string(),
                public_key: "pub".to_string(),
                source_url: "https://example.com/cr.tar.gz".to_string(),
                scan_result: ScanVerdict::Clean,
            }],
            install_count: 42,
            rating: 4.5,
            report_count: 0,
            yanked: false,
        };

        let json = serde_json::to_string_pretty(&meta).unwrap();
        std::fs::write(skill_dir.join("code-reviewer.json"), json).unwrap();

        let client = IndexClient::new("https://example.com/index.git", dir.path());
        let entries = client.read_all_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "code-reviewer");
    }

    #[test]
    fn test_search_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");
        let skill_dir = index_dir.join("co");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let meta = IndexMeta {
            name: "code-reviewer".to_string(),
            versions: vec![],
            install_count: 0,
            rating: 0.0,
            report_count: 0,
            yanked: false,
        };
        std::fs::write(
            skill_dir.join("code-reviewer.json"),
            serde_json::to_string(&meta).unwrap(),
        )
        .unwrap();

        let client = IndexClient::new("https://example.com", dir.path());
        let results = client.search("code", None, None).unwrap();
        assert_eq!(results.len(), 1);

        let no_results = client.search("zzzzz", None, None).unwrap();
        assert!(no_results.is_empty());
    }

    #[test]
    fn test_read_entry() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");
        let skill_dir = index_dir.join("co");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let meta = IndexMeta {
            name: "code-reviewer".to_string(),
            versions: vec![IndexEntry {
                name: "code-reviewer".to_string(),
                version: "1.0.0".to_string(),
                checksum: "abc".to_string(),
                signature: "sig".to_string(),
                public_key: "pub".to_string(),
                source_url: "https://example.com/cr.tar.gz".to_string(),
                scan_result: ScanVerdict::Clean,
            }],
            install_count: 0,
            rating: 0.0,
            report_count: 0,
            yanked: false,
        };
        // Write at the expected path: index/co/code-reviewer.json
        std::fs::write(
            skill_dir.join("code-reviewer.json"),
            serde_json::to_string(&meta).unwrap(),
        )
        .unwrap();

        let client = IndexClient::new("https://example.com", dir.path());
        let entry = client.get_entry("code-reviewer", "1.0.0").unwrap();
        assert_eq!(entry.name, "code-reviewer");
        assert_eq!(entry.version, "1.0.0");
    }

    #[test]
    fn test_read_entry_not_found() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("index")).unwrap();
        let client = IndexClient::new("https://example.com", dir.path());
        let result = client.read_entry("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_version_latest() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");
        let skill_dir = index_dir.join("co");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let meta = IndexMeta {
            name: "code-reviewer".to_string(),
            versions: vec![
                IndexEntry {
                    name: "code-reviewer".to_string(),
                    version: "2.0.0".to_string(),
                    checksum: "abc".to_string(),
                    signature: "sig".to_string(),
                    public_key: "pub".to_string(),
                    source_url: "url".to_string(),
                    scan_result: ScanVerdict::Clean,
                },
                IndexEntry {
                    name: "code-reviewer".to_string(),
                    version: "1.0.0".to_string(),
                    checksum: "def".to_string(),
                    signature: "sig".to_string(),
                    public_key: "pub".to_string(),
                    source_url: "url".to_string(),
                    scan_result: ScanVerdict::Clean,
                },
            ],
            install_count: 0,
            rating: 0.0,
            report_count: 0,
            yanked: false,
        };
        std::fs::write(
            skill_dir.join("code-reviewer.json"),
            serde_json::to_string(&meta).unwrap(),
        )
        .unwrap();

        let client = IndexClient::new("https://example.com", dir.path());
        let version = client.resolve_version("code-reviewer", "latest").unwrap();
        assert_eq!(version, "2.0.0");

        let exact = client.resolve_version("code-reviewer", "1.0.0").unwrap();
        assert_eq!(exact, "1.0.0");
    }

    #[test]
    fn test_resolve_version_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");
        let skill_dir = index_dir.join("co");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let meta = IndexMeta {
            name: "code-reviewer".to_string(),
            versions: vec![IndexEntry {
                name: "code-reviewer".to_string(),
                version: "1.0.0".to_string(),
                checksum: "abc".to_string(),
                signature: "sig".to_string(),
                public_key: "pub".to_string(),
                source_url: "url".to_string(),
                scan_result: ScanVerdict::Clean,
            }],
            install_count: 0,
            rating: 0.0,
            report_count: 0,
            yanked: false,
        };
        std::fs::write(
            skill_dir.join("code-reviewer.json"),
            serde_json::to_string(&meta).unwrap(),
        )
        .unwrap();

        let client = IndexClient::new("https://example.com", dir.path());
        let result = client.resolve_version("code-reviewer", "99.0.0");
        assert!(result.is_err());
    }
}
