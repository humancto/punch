//! Git-index types and parsing for the skills marketplace.
//!
//! The index follows the crates.io-index pattern: a Git repository where
//! each skill has a JSON metadata file routed by two-character prefix
//! (e.g., `co/code-reviewer/`).

use serde::{Deserialize, Serialize};

use punch_types::PunchResult;

// ---------------------------------------------------------------------------
// Index types
// ---------------------------------------------------------------------------

/// A single version entry in the Git index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    /// Skill name (slug format).
    pub name: String,
    /// Semantic version.
    pub version: String,
    /// SHA-256 checksum of the tarball.
    pub checksum: String,
    /// Ed25519 signature of the checksum (hex-encoded).
    pub signature: String,
    /// Ed25519 public key of the signer (hex-encoded).
    pub public_key: String,
    /// URL to fetch the skill tarball from.
    pub source_url: String,
    /// Result of the security scan at publish time.
    pub scan_result: ScanVerdict,
}

/// Aggregate metadata for a skill across all versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMeta {
    /// Skill name.
    pub name: String,
    /// All published versions (newest first).
    pub versions: Vec<IndexEntry>,
    /// Total install count across all versions.
    pub install_count: u64,
    /// Average community rating (0.0–5.0).
    pub rating: f64,
    /// Number of abuse reports.
    pub report_count: u64,
    /// Whether this skill has been yanked (hidden from search).
    pub yanked: bool,
}

/// Security scan verdict for a skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanVerdict {
    /// No issues found.
    Clean,
    /// Non-blocking issues found (informational warnings).
    Warning(Vec<ScanFinding>),
    /// Blocking issues found — skill must not be installed.
    Rejected(Vec<ScanFinding>),
}

/// A single finding from the security scanner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanFinding {
    /// Severity: "critical", "warning", or "info".
    pub severity: String,
    /// The pattern rule that matched.
    pub pattern: String,
    /// Line number where the finding was detected (1-based).
    pub line: usize,
    /// Human-readable description of the issue.
    pub description: String,
}

// ---------------------------------------------------------------------------
// Name validation
// ---------------------------------------------------------------------------

/// Validate a skill name follows the slug format: `[a-z0-9][a-z0-9-]{2,63}`.
///
/// Rules:
/// - 3–64 characters
/// - Lowercase alphanumeric + hyphens only
/// - Must start with alphanumeric
/// - Must not end with a hyphen
/// - No consecutive hyphens
pub fn validate_skill_name(name: &str) -> PunchResult<()> {
    if name.len() < 3 {
        return Err(punch_types::PunchError::Config(format!(
            "skill name '{}' is too short (minimum 3 characters)",
            name
        )));
    }
    if name.len() > 64 {
        return Err(punch_types::PunchError::Config(format!(
            "skill name '{}' is too long (maximum 64 characters)",
            name
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(punch_types::PunchError::Config(format!(
            "skill name '{}' contains invalid characters (only lowercase alphanumeric and hyphens allowed)",
            name
        )));
    }
    if !name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return Err(punch_types::PunchError::Config(format!(
            "skill name '{}' must start with a letter or digit",
            name
        )));
    }
    if name.ends_with('-') {
        return Err(punch_types::PunchError::Config(format!(
            "skill name '{}' must not end with a hyphen",
            name
        )));
    }
    if name.contains("--") {
        return Err(punch_types::PunchError::Config(format!(
            "skill name '{}' must not contain consecutive hyphens",
            name
        )));
    }
    Ok(())
}

/// Compute the index path for a skill name using two-char prefix routing.
///
/// Examples:
/// - `"co"` → `"co/co"`
/// - `"code-reviewer"` → `"co/code-reviewer"`
/// - `"ab"` prefix for `"abc"` → `"ab/abc"`
pub fn index_path_for_name(name: &str) -> String {
    match name.len() {
        0 => String::new(),
        1 => format!("1/{}", name),
        2 => format!("2/{}", name),
        3 => format!("3/{}/{}", &name[..1], name),
        _ => format!("{}/{}", &name[..2], name),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_skill_name_valid() {
        assert!(validate_skill_name("code-reviewer").is_ok());
        assert!(validate_skill_name("abc").is_ok());
        assert!(validate_skill_name("my-awesome-skill").is_ok());
        assert!(validate_skill_name("a1b2c3").is_ok());
        assert!(validate_skill_name("skill123").is_ok());
    }

    #[test]
    fn test_validate_skill_name_too_short() {
        assert!(validate_skill_name("ab").is_err());
        assert!(validate_skill_name("a").is_err());
        assert!(validate_skill_name("").is_err());
    }

    #[test]
    fn test_validate_skill_name_too_long() {
        let long_name = "a".repeat(65);
        assert!(validate_skill_name(&long_name).is_err());

        let max_name = "a".repeat(64);
        assert!(validate_skill_name(&max_name).is_ok());
    }

    #[test]
    fn test_validate_skill_name_invalid_chars() {
        assert!(validate_skill_name("Code-Reviewer").is_err()); // uppercase
        assert!(validate_skill_name("my_skill").is_err()); // underscore
        assert!(validate_skill_name("my skill").is_err()); // space
        assert!(validate_skill_name("my.skill").is_err()); // dot
    }

    #[test]
    fn test_validate_skill_name_must_start_alphanumeric() {
        assert!(validate_skill_name("-bad-start").is_err());
    }

    #[test]
    fn test_validate_skill_name_must_not_end_hyphen() {
        assert!(validate_skill_name("bad-end-").is_err());
    }

    #[test]
    fn test_validate_skill_name_no_consecutive_hyphens() {
        assert!(validate_skill_name("bad--name").is_err());
    }

    #[test]
    fn test_index_path_for_name() {
        assert_eq!(index_path_for_name("code-reviewer"), "co/code-reviewer");
        assert_eq!(index_path_for_name("abc"), "3/a/abc");
        assert_eq!(index_path_for_name("ab"), "2/ab");
        assert_eq!(index_path_for_name("a"), "1/a");
        assert_eq!(index_path_for_name("my-tool"), "my/my-tool");
    }

    #[test]
    fn test_scan_verdict_serde() {
        let clean = ScanVerdict::Clean;
        let json = serde_json::to_string(&clean).unwrap();
        let restored: ScanVerdict = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, ScanVerdict::Clean);
    }

    #[test]
    fn test_scan_finding_serde() {
        let finding = ScanFinding {
            severity: "critical".to_string(),
            pattern: "pipe_to_shell".to_string(),
            line: 42,
            description: "curl piped to bash detected".to_string(),
        };
        let json = serde_json::to_string(&finding).unwrap();
        let restored: ScanFinding = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, finding);
    }

    #[test]
    fn test_scan_verdict_warning_serde() {
        let findings = vec![ScanFinding {
            severity: "warning".to_string(),
            pattern: "sudo_usage".to_string(),
            line: 10,
            description: "sudo command detected".to_string(),
        }];
        let verdict = ScanVerdict::Warning(findings.clone());
        let json = serde_json::to_string(&verdict).unwrap();
        let restored: ScanVerdict = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, ScanVerdict::Warning(findings));
    }

    #[test]
    fn test_index_entry_serde() {
        let entry = IndexEntry {
            name: "code-reviewer".to_string(),
            version: "1.0.0".to_string(),
            checksum: "abcd1234".to_string(),
            signature: "deadbeef".to_string(),
            public_key: "cafebabe".to_string(),
            source_url: "https://example.com/skill.tar.gz".to_string(),
            scan_result: ScanVerdict::Clean,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let restored: IndexEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "code-reviewer");
        assert_eq!(restored.version, "1.0.0");
        assert_eq!(restored.scan_result, ScanVerdict::Clean);
    }

    #[test]
    fn test_index_meta_serde() {
        let meta = IndexMeta {
            name: "test-skill".to_string(),
            versions: vec![],
            install_count: 42,
            rating: 4.5,
            report_count: 0,
            yanked: false,
        };
        let json = serde_json::to_string(&meta).unwrap();
        let restored: IndexMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.name, "test-skill");
        assert_eq!(restored.install_count, 42);
        assert!(!restored.yanked);
    }
}
