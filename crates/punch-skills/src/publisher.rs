//! Publishing pipeline for the skills marketplace.
//!
//! Handles validation, tarball creation, checksumming, and signing of skills
//! before they are submitted to the index repository.

use std::path::Path;

use sha2::{Digest, Sha256};
use tracing::debug;

use punch_types::PunchResult;
use punch_types::signing::SigningKeyPair;

use crate::loader::{SkillFrontmatter, parse_skill_md};
use crate::registry::{IndexEntry, ScanVerdict, validate_skill_name};
use crate::scanner::SkillScanner;

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a skill directory is ready for publishing.
///
/// Returns a list of validation error messages (empty = valid).
pub fn validate_for_publish(dir: &Path) -> Vec<String> {
    let mut errors = Vec::new();

    let skill_path = dir.join("SKILL.md");
    if !skill_path.exists() {
        errors.push("SKILL.md not found in directory".to_string());
        return errors;
    }

    let content = match std::fs::read_to_string(&skill_path) {
        Ok(c) => c,
        Err(e) => {
            errors.push(format!("failed to read SKILL.md: {}", e));
            return errors;
        }
    };

    let frontmatter = match parse_skill_md(&content) {
        Ok((fm, _body)) => fm,
        Err(e) => {
            errors.push(format!("invalid SKILL.md format: {}", e));
            return errors;
        }
    };

    // Validate name slug
    if let Err(e) = validate_skill_name(&frontmatter.name) {
        errors.push(format!("invalid skill name: {}", e));
    }

    // Validate version is semver-ish
    if !is_valid_semver(&frontmatter.version) {
        errors.push(format!(
            "invalid version '{}' — must be semver (e.g., 1.0.0)",
            frontmatter.version
        ));
    }

    // Validate description is present
    if frontmatter.description.is_empty() {
        errors.push("description is required".to_string());
    }

    // Validate author is present
    if frontmatter.author.is_empty() {
        errors.push("author is required".to_string());
    }

    // Check total size of publishable files
    if let Ok(size) = publishable_size(dir)
        && size > 100 * 1024
    {
        errors.push(format!(
            "skill package is too large ({} bytes, maximum 100KB)",
            size
        ));
    }

    errors
}

/// Basic semver validation (major.minor.patch with optional pre-release).
fn is_valid_semver(version: &str) -> bool {
    let parts: Vec<&str> = version.split('-').collect();
    let core = parts[0];
    let nums: Vec<&str> = core.split('.').collect();
    if nums.len() != 3 {
        return false;
    }
    nums.iter().all(|n| n.parse::<u64>().is_ok())
}

/// Calculate total size of publishable files in the directory.
fn publishable_size(dir: &Path) -> PunchResult<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if path.is_file()
            && let Some(ext) = path.extension().and_then(|e| e.to_str())
            && matches!(ext, "md" | "txt")
        {
            total += std::fs::metadata(&path)?.len();
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// Tarball creation
// ---------------------------------------------------------------------------

/// Create a tar.gz archive of publishable files in the skill directory.
///
/// Only includes `.md` and `.txt` files (skills are text-only).
/// Maximum size enforced at 100KB.
pub fn create_tarball(dir: &Path) -> PunchResult<Vec<u8>> {
    let buf = Vec::new();
    let encoder = flate2::write::GzEncoder::new(buf, flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);

    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if path.is_file()
            && let Some(ext) = path.extension().and_then(|e| e.to_str())
            && matches!(ext, "md" | "txt")
        {
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
            debug!(file = file_name, "adding to tarball");
            archive
                .append_path_with_name(&path, file_name)
                .map_err(|e| {
                    punch_types::PunchError::Config(format!(
                        "failed to add {} to tarball: {}",
                        file_name, e
                    ))
                })?;
        }
    }

    let encoder = archive.into_inner().map_err(|e| {
        punch_types::PunchError::Config(format!("failed to finalize tarball: {}", e))
    })?;
    let compressed = encoder.finish().map_err(|e| {
        punch_types::PunchError::Config(format!("failed to compress tarball: {}", e))
    })?;

    if compressed.len() > 100 * 1024 {
        return Err(punch_types::PunchError::Config(format!(
            "compressed tarball is too large ({} bytes, maximum 100KB)",
            compressed.len()
        )));
    }

    Ok(compressed)
}

// ---------------------------------------------------------------------------
// Checksum & Signing
// ---------------------------------------------------------------------------

/// Compute SHA-256 checksum of data, returned as hex string.
pub fn compute_checksum(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    result.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Sign a checksum string with an Ed25519 keypair.
///
/// Returns hex-encoded 64-byte signature.
pub fn sign_checksum(checksum: &str, key: &SigningKeyPair) -> String {
    let sig = key.sign(checksum.as_bytes());
    sig.to_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

// ---------------------------------------------------------------------------
// Index entry building
// ---------------------------------------------------------------------------

/// Build an index entry from publish artifacts.
pub fn build_index_entry(
    frontmatter: &SkillFrontmatter,
    checksum: &str,
    signature: &str,
    public_key: &str,
    source_url: &str,
    scan_result: ScanVerdict,
) -> IndexEntry {
    IndexEntry {
        name: frontmatter.name.clone(),
        version: frontmatter.version.clone(),
        checksum: checksum.to_string(),
        signature: signature.to_string(),
        public_key: public_key.to_string(),
        source_url: source_url.to_string(),
        scan_result,
    }
}

// ---------------------------------------------------------------------------
// Dry run
// ---------------------------------------------------------------------------

/// Perform a full publish dry run: validate, scan, create tarball, checksum.
///
/// Returns a summary report as a string. Does not actually publish.
pub fn dry_run(dir: &Path) -> PunchResult<String> {
    let mut report = String::new();

    // Validate
    let errors = validate_for_publish(dir);
    if !errors.is_empty() {
        report.push_str("Validation FAILED:\n");
        for e in &errors {
            report.push_str(&format!("  - {}\n", e));
        }
        return Ok(report);
    }
    report.push_str("Validation: PASSED\n");

    // Parse frontmatter
    let skill_path = dir.join("SKILL.md");
    let content = std::fs::read_to_string(&skill_path)?;
    let (frontmatter, _body) = parse_skill_md(&content)?;
    report.push_str(&format!("  Name: {}\n", frontmatter.name));
    report.push_str(&format!("  Version: {}\n", frontmatter.version));
    report.push_str(&format!("  Author: {}\n", frontmatter.author));

    // Security scan
    let scanner = SkillScanner::new();
    let verdict = scanner.scan(&content);
    match &verdict {
        ScanVerdict::Clean => report.push_str("Security scan: CLEAN\n"),
        ScanVerdict::Warning(findings) => {
            report.push_str(&format!("Security scan: {} WARNING(s)\n", findings.len()));
            for f in findings {
                report.push_str(&format!(
                    "  - [{}] L{}: {}\n",
                    f.severity, f.line, f.description
                ));
            }
        }
        ScanVerdict::Rejected(findings) => {
            report.push_str(&format!(
                "Security scan: REJECTED ({} finding(s))\n",
                findings.len()
            ));
            for f in findings {
                report.push_str(&format!(
                    "  - [{}] L{}: {}\n",
                    f.severity, f.line, f.description
                ));
            }
        }
    }

    // Create tarball
    let tarball = create_tarball(dir)?;
    let checksum = compute_checksum(&tarball);
    report.push_str(&format!("Tarball size: {} bytes\n", tarball.len()));
    report.push_str(&format!("Checksum: {}\n", checksum));

    report.push_str("\nDry run complete. Ready to publish.\n");
    Ok(report)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_skill_dir(name: &str, version: &str, author: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let content = format!(
            "---\nname: {}\nversion: {}\ndescription: A test skill\nauthor: {}\ncategory: test\ntags: [test]\n---\n\n# Test Skill\n\nThis is a test.\n",
            name, version, author
        );
        fs::write(dir.path().join("SKILL.md"), content).unwrap();
        dir
    }

    #[test]
    fn test_validate_valid_skill() {
        let dir = make_skill_dir("test-skill", "1.0.0", "Test Author");
        let errors = validate_for_publish(dir.path());
        assert!(errors.is_empty(), "expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_validate_missing_skill_md() {
        let dir = tempfile::tempdir().unwrap();
        let errors = validate_for_publish(dir.path());
        assert!(!errors.is_empty());
        assert!(errors[0].contains("SKILL.md not found"));
    }

    #[test]
    fn test_validate_invalid_name() {
        let dir = make_skill_dir("Bad_Name", "1.0.0", "Author");
        let errors = validate_for_publish(dir.path());
        assert!(!errors.is_empty());
    }

    #[test]
    fn test_validate_invalid_version() {
        let dir = make_skill_dir("valid-name", "not-a-version", "Author");
        let errors = validate_for_publish(dir.path());
        assert!(errors.iter().any(|e| e.contains("version")));
    }

    #[test]
    fn test_validate_missing_description() {
        let dir = tempfile::tempdir().unwrap();
        let content = "---\nname: test-skill\nversion: 1.0.0\nauthor: test\n---\n\nBody here.";
        fs::write(dir.path().join("SKILL.md"), content).unwrap();
        let errors = validate_for_publish(dir.path());
        assert!(errors.iter().any(|e| e.contains("description")));
    }

    #[test]
    fn test_validate_missing_author() {
        let dir = tempfile::tempdir().unwrap();
        let content =
            "---\nname: test-skill\nversion: 1.0.0\ndescription: A test\n---\n\nBody here.";
        fs::write(dir.path().join("SKILL.md"), content).unwrap();
        let errors = validate_for_publish(dir.path());
        assert!(errors.iter().any(|e| e.contains("author")));
    }

    #[test]
    fn test_is_valid_semver() {
        assert!(is_valid_semver("1.0.0"));
        assert!(is_valid_semver("0.1.0"));
        assert!(is_valid_semver("10.20.30"));
        assert!(is_valid_semver("1.0.0-beta"));
        assert!(!is_valid_semver("1.0"));
        assert!(!is_valid_semver("latest"));
        assert!(!is_valid_semver("1.0.0.0"));
    }

    #[test]
    fn test_create_tarball() {
        let dir = make_skill_dir("tarball-test", "1.0.0", "Author");
        let tarball = create_tarball(dir.path()).unwrap();
        assert!(!tarball.is_empty());
    }

    #[test]
    fn test_compute_checksum() {
        let data = b"hello world";
        let checksum = compute_checksum(data);
        assert_eq!(checksum.len(), 64); // SHA-256 = 32 bytes = 64 hex chars
        assert!(checksum.chars().all(|c| c.is_ascii_hexdigit()));

        // Same input = same checksum
        assert_eq!(compute_checksum(data), checksum);

        // Different input = different checksum
        assert_ne!(compute_checksum(b"different"), checksum);
    }

    #[test]
    fn test_sign_checksum() {
        let (keypair, _vk) = punch_types::signing::generate_keypair();
        let checksum = compute_checksum(b"test data");
        let signature = sign_checksum(&checksum, &keypair);
        assert_eq!(signature.len(), 128); // 64 bytes = 128 hex chars
    }

    #[test]
    fn test_build_index_entry() {
        let fm = SkillFrontmatter {
            name: "test-skill".to_string(),
            version: "1.0.0".to_string(),
            description: "A test".to_string(),
            author: "Author".to_string(),
            category: "test".to_string(),
            tags: vec![],
            tools: vec![],
            requires: vec![],
        };
        let entry = build_index_entry(
            &fm,
            "abc123",
            "sig456",
            "pub789",
            "https://example.com/skill.tar.gz",
            ScanVerdict::Clean,
        );
        assert_eq!(entry.name, "test-skill");
        assert_eq!(entry.version, "1.0.0");
        assert_eq!(entry.checksum, "abc123");
        assert_eq!(entry.scan_result, ScanVerdict::Clean);
    }

    #[test]
    fn test_dry_run_valid_skill() {
        let dir = make_skill_dir("dry-run-test", "1.0.0", "Author");
        let report = dry_run(dir.path()).unwrap();
        assert!(report.contains("Validation: PASSED"));
        assert!(report.contains("dry-run-test"));
        assert!(report.contains("Checksum:"));
    }

    #[test]
    fn test_dry_run_invalid_skill() {
        let dir = tempfile::tempdir().unwrap();
        let report = dry_run(dir.path()).unwrap();
        assert!(report.contains("Validation FAILED"));
    }

    #[test]
    fn test_tarball_only_includes_text() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("SKILL.md"),
            "---\nname: test\nversion: 1.0.0\n---\n\nBody.",
        )
        .unwrap();
        fs::write(dir.path().join("notes.txt"), "extra notes").unwrap();
        fs::write(dir.path().join("binary.bin"), vec![0u8; 100]).unwrap();
        fs::write(dir.path().join("image.png"), vec![0u8; 100]).unwrap();

        let tarball = create_tarball(dir.path()).unwrap();
        // Verify by decompressing and checking entries
        let decoder = flate2::read::GzDecoder::new(&tarball[..]);
        let mut archive = tar::Archive::new(decoder);
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| e.path().ok().map(|p| p.to_string_lossy().to_string()))
            .collect();
        assert!(names.contains(&"SKILL.md".to_string()));
        assert!(names.contains(&"notes.txt".to_string()));
        assert!(!names.iter().any(|n| n.ends_with(".bin")));
        assert!(!names.iter().any(|n| n.ends_with(".png")));
    }
}
