//! Install-time verification for marketplace skills.
//!
//! Before a skill is installed, we verify:
//! 1. SHA-256 checksum matches the index entry
//! 2. Ed25519 signature is valid for the publisher's public key
//! 3. Security scan passes (no critical findings)

use punch_types::PunchResult;
use punch_types::signing::{verify_manifest, verifying_key_from_hex};

use crate::publisher::compute_checksum;
use crate::registry::{IndexEntry, ScanVerdict};
use crate::scanner::SkillScanner;

// ---------------------------------------------------------------------------
// Verification functions
// ---------------------------------------------------------------------------

/// Verify that data matches the expected SHA-256 checksum.
pub fn verify_checksum(data: &[u8], expected: &str) -> bool {
    let actual = compute_checksum(data);
    actual == expected
}

/// Verify an Ed25519 signature against a checksum and public key.
///
/// The public key is hex-encoded (64 hex chars = 32 bytes).
/// The signature is hex-encoded (128 hex chars = 64 bytes).
pub fn verify_signature(checksum: &str, signature: &str, public_key: &str) -> PunchResult<()> {
    let vk = verifying_key_from_hex(public_key).map_err(|e| {
        punch_types::PunchError::Config(format!("invalid publisher public key: {}", e))
    })?;

    let valid = verify_manifest(&vk, checksum.as_bytes(), signature).map_err(|e| {
        punch_types::PunchError::Config(format!("signature verification error: {}", e))
    })?;

    if !valid {
        return Err(punch_types::PunchError::Config(
            "signature verification failed — skill may have been tampered with".to_string(),
        ));
    }

    Ok(())
}

/// Full verification pipeline: checksum + signature + security scan.
///
/// Returns the scan verdict (may be Warning for non-critical findings).
pub fn verify_and_scan(data: &[u8], entry: &IndexEntry) -> PunchResult<ScanVerdict> {
    // Step 1: Verify checksum
    if !verify_checksum(data, &entry.checksum) {
        return Err(punch_types::PunchError::Config(
            "checksum mismatch — downloaded skill data does not match index entry".to_string(),
        ));
    }

    // Step 2: Verify signature
    verify_signature(&entry.checksum, &entry.signature, &entry.public_key)?;

    // Step 3: Decompress and scan content
    let content = decompress_and_extract_skill(data)?;
    let scanner = SkillScanner::new();
    let verdict = scanner.scan(&content);

    if let ScanVerdict::Rejected(ref findings) = verdict {
        return Err(punch_types::PunchError::Config(format!(
            "security scan rejected skill with {} critical finding(s): {}",
            findings.len(),
            findings
                .iter()
                .map(|f| f.description.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }

    Ok(verdict)
}

/// Extract SKILL.md content from a tar.gz archive.
fn decompress_and_extract_skill(data: &[u8]) -> PunchResult<String> {
    let decoder = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive
        .entries()
        .map_err(|e| punch_types::PunchError::Config(format!("failed to read tarball: {}", e)))?
    {
        let mut entry = entry.map_err(|e| {
            punch_types::PunchError::Config(format!("failed to read tarball entry: {}", e))
        })?;

        let path = entry
            .path()
            .map_err(|e| {
                punch_types::PunchError::Config(format!("invalid path in tarball: {}", e))
            })?
            .to_path_buf();

        if path.file_name().is_some_and(|f| f == "SKILL.md") {
            let mut content = String::new();
            std::io::Read::read_to_string(&mut entry, &mut content).map_err(|e| {
                punch_types::PunchError::Config(format!(
                    "failed to read SKILL.md from tarball: {}",
                    e
                ))
            })?;
            return Ok(content);
        }
    }

    Err(punch_types::PunchError::Config(
        "SKILL.md not found in tarball".to_string(),
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publisher::{create_tarball, sign_checksum};
    use punch_types::signing::generate_keypair;
    use std::fs;

    #[test]
    fn test_verify_checksum_valid() {
        let data = b"hello world";
        let checksum = compute_checksum(data);
        assert!(verify_checksum(data, &checksum));
    }

    #[test]
    fn test_verify_checksum_invalid() {
        let data = b"hello world";
        assert!(!verify_checksum(data, "wrong_checksum"));
    }

    #[test]
    fn test_verify_signature_valid() {
        let (keypair, _vk) = generate_keypair();
        let checksum = compute_checksum(b"test data");
        let signature = sign_checksum(&checksum, &keypair);
        let public_key = keypair.verifying_key_hex();

        let result = verify_signature(&checksum, &signature, &public_key);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_signature_wrong_key() {
        let (keypair, _vk) = generate_keypair();
        let (keypair2, _vk2) = generate_keypair();
        let checksum = compute_checksum(b"test data");
        let signature = sign_checksum(&checksum, &keypair);
        let wrong_public_key = keypair2.verifying_key_hex();

        let result = verify_signature(&checksum, &signature, &wrong_public_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_signature_tampered_checksum() {
        let (keypair, _vk) = generate_keypair();
        let checksum = compute_checksum(b"test data");
        let signature = sign_checksum(&checksum, &keypair);
        let public_key = keypair.verifying_key_hex();

        let result = verify_signature("tampered_checksum", &signature, &public_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_and_scan_full_pipeline() {
        // Create a valid skill
        let dir = tempfile::tempdir().unwrap();
        let content = "---\nname: test-skill\nversion: 1.0.0\ndescription: A test\nauthor: Test\n---\n\n# Test\n\nClean skill content.\n";
        fs::write(dir.path().join("SKILL.md"), content).unwrap();

        // Create tarball
        let tarball = create_tarball(dir.path()).unwrap();

        // Sign it
        let (keypair, _vk) = generate_keypair();
        let checksum = compute_checksum(&tarball);
        let signature = sign_checksum(&checksum, &keypair);

        let entry = IndexEntry {
            name: "test-skill".to_string(),
            version: "1.0.0".to_string(),
            checksum,
            signature,
            public_key: keypair.verifying_key_hex(),
            source_url: "https://example.com/test.tar.gz".to_string(),
            scan_result: ScanVerdict::Clean,
        };

        let verdict = verify_and_scan(&tarball, &entry).unwrap();
        assert_eq!(verdict, ScanVerdict::Clean);
    }

    #[test]
    fn test_verify_and_scan_checksum_mismatch() {
        let (keypair, _vk) = generate_keypair();
        let entry = IndexEntry {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            checksum: "wrong_checksum".to_string(),
            signature: "sig".to_string(),
            public_key: keypair.verifying_key_hex(),
            source_url: "https://example.com".to_string(),
            scan_result: ScanVerdict::Clean,
        };

        let result = verify_and_scan(b"some data", &entry);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("checksum mismatch")
        );
    }

    #[test]
    fn test_verify_signature_invalid_public_key() {
        let result = verify_signature("checksum", "sig", "not_valid_hex");
        assert!(result.is_err());
    }
}
