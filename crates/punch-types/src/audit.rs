//! Tamper-evident audit log for the Punch Agent Combat System.
//!
//! Every security-relevant action in the ring is recorded as an [`AuditEntry`]
//! whose SHA-256 hash incorporates the previous entry's hash, forming a Merkle
//! hash chain — the fight record that cannot be rewritten after the fact.
//!
//! Think of it as the official bout log: once a punch is thrown, the record is
//! sealed and any attempt to alter history breaks the chain.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// AuditAction — what happened in the ring
// ---------------------------------------------------------------------------

/// A security-relevant action recorded in the bout log.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum AuditAction {
    /// A tool (move) was executed by a fighter.
    ToolExecuted {
        tool: String,
        fighter_id: String,
        success: bool,
    },
    /// A tool (move) was blocked before execution.
    ToolBlocked {
        tool: String,
        fighter_id: String,
        reason: String,
    },
    /// An approval request was raised for a risky move.
    ApprovalRequested {
        tool: String,
        fighter_id: String,
        risk_level: String,
    },
    /// Approval was granted for a move.
    ApprovalGranted { tool: String, fighter_id: String },
    /// Approval was denied for a move.
    ApprovalDenied {
        tool: String,
        fighter_id: String,
        reason: String,
    },
    /// A capability was granted to a fighter.
    CapabilityGranted {
        capability: String,
        fighter_id: String,
        granted_by: String,
    },
    /// A capability request was denied.
    CapabilityDenied {
        capability: String,
        fighter_id: String,
    },
    /// Tainted data was detected in the ring.
    TaintDetected {
        source: String,
        value_preview: String,
        severity: String,
    },
    /// A shell-bleed injection pattern was detected.
    ShellBleedDetected {
        command_preview: String,
        pattern: String,
        severity: String,
    },
    /// A new fighter entered the ring.
    FighterSpawned { fighter_id: String, name: String },
    /// A fighter was knocked out (terminated).
    FighterKilled { fighter_id: String, name: String },
    /// A new bout (session) started.
    SessionStarted { bout_id: String, fighter_id: String },
    /// A configuration value was changed.
    ConfigChanged {
        key: String,
        old_preview: String,
        new_preview: String,
    },
}

impl AuditAction {
    /// Returns the action type name used for filtering the bout log.
    pub fn type_name(&self) -> &'static str {
        match self {
            AuditAction::ToolExecuted { .. } => "ToolExecuted",
            AuditAction::ToolBlocked { .. } => "ToolBlocked",
            AuditAction::ApprovalRequested { .. } => "ApprovalRequested",
            AuditAction::ApprovalGranted { .. } => "ApprovalGranted",
            AuditAction::ApprovalDenied { .. } => "ApprovalDenied",
            AuditAction::CapabilityGranted { .. } => "CapabilityGranted",
            AuditAction::CapabilityDenied { .. } => "CapabilityDenied",
            AuditAction::TaintDetected { .. } => "TaintDetected",
            AuditAction::ShellBleedDetected { .. } => "ShellBleedDetected",
            AuditAction::FighterSpawned { .. } => "FighterSpawned",
            AuditAction::FighterKilled { .. } => "FighterKilled",
            AuditAction::SessionStarted { .. } => "SessionStarted",
            AuditAction::ConfigChanged { .. } => "ConfigChanged",
        }
    }
}

// ---------------------------------------------------------------------------
// AuditEntry — a single record in the fight log
// ---------------------------------------------------------------------------

/// A single, hash-chained record in the bout log.
///
/// Each entry's [`hash`](AuditEntry::hash) is computed over its content *and*
/// the previous entry's hash, making the log tamper-evident.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unique identifier for this record.
    pub id: Uuid,
    /// Monotonically increasing sequence number within the bout log.
    pub sequence: u64,
    /// When the action occurred.
    pub timestamp: DateTime<Utc>,
    /// What happened.
    pub action: AuditAction,
    /// Who performed the action (fighter ID, "system", "user", etc.).
    pub actor: String,
    /// Additional context attached to this record.
    pub metadata: serde_json::Value,
    /// Hex-encoded SHA-256 hash of the previous entry (empty string for the
    /// genesis entry).
    pub prev_hash: String,
    /// Hex-encoded SHA-256 hash of this entry's content plus `prev_hash`.
    pub hash: String,
}

// ---------------------------------------------------------------------------
// AuditVerifyError — chain integrity violations
// ---------------------------------------------------------------------------

/// Errors detected when verifying the integrity of the bout log's hash chain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuditVerifyError {
    /// The stored hash for an entry does not match the recomputed hash.
    HashMismatch {
        sequence: u64,
        expected: String,
        actual: String,
    },
    /// The `prev_hash` of an entry does not point to the preceding entry's hash.
    ChainBroken {
        sequence: u64,
        expected_prev: String,
        actual_prev: String,
    },
    /// A gap was found in the sequence numbering.
    SequenceGap { expected: u64, actual: u64 },
}

impl std::fmt::Display for AuditVerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuditVerifyError::HashMismatch {
                sequence,
                expected,
                actual,
            } => write!(
                f,
                "hash mismatch at sequence {sequence}: expected {expected}, got {actual}"
            ),
            AuditVerifyError::ChainBroken {
                sequence,
                expected_prev,
                actual_prev,
            } => write!(
                f,
                "chain broken at sequence {sequence}: expected prev_hash {expected_prev}, got {actual_prev}"
            ),
            AuditVerifyError::SequenceGap { expected, actual } => {
                write!(f, "sequence gap: expected {expected}, got {actual}")
            }
        }
    }
}

impl std::error::Error for AuditVerifyError {}

// ---------------------------------------------------------------------------
// Hash computation — the seal on each record
// ---------------------------------------------------------------------------

/// Compute the deterministic SHA-256 hash for an audit entry.
///
/// The hash covers: `sequence|timestamp_rfc3339|action_json|actor|metadata_json|prev_hash`
fn compute_entry_hash(entry: &AuditEntry) -> String {
    let action_json = serde_json::to_string(&entry.action).unwrap_or_default();
    let metadata_json = serde_json::to_string(&entry.metadata).unwrap_or_default();
    let timestamp_rfc3339 = entry.timestamp.to_rfc3339();

    let preimage = format!(
        "{}|{}|{}|{}|{}|{}",
        entry.sequence, timestamp_rfc3339, action_json, entry.actor, metadata_json, entry.prev_hash
    );

    let mut hasher = Sha256::new();
    hasher.update(preimage.as_bytes());
    format!("{:x}", hasher.finalize())
}

// ---------------------------------------------------------------------------
// AuditLog — the append-only bout log
// ---------------------------------------------------------------------------

/// An append-only, tamper-evident fight record.
///
/// Entries form a Merkle hash chain: each entry's hash incorporates the
/// previous entry's hash. Call [`verify_chain`](AuditLog::verify_chain) to
/// confirm the log has not been altered since it was written.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLog {
    entries: Vec<AuditEntry>,
    next_sequence: u64,
}

impl AuditLog {
    /// Create a fresh, empty bout log.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_sequence: 0,
        }
    }

    /// Record a new action in the bout log.
    ///
    /// The entry is appended with a hash that chains to the previous entry,
    /// making the entire log tamper-evident.
    pub fn append(
        &mut self,
        action: AuditAction,
        actor: &str,
        metadata: serde_json::Value,
    ) -> &AuditEntry {
        let prev_hash = self
            .entries
            .last()
            .map(|e| e.hash.clone())
            .unwrap_or_default();

        let sequence = self.next_sequence;

        // Build the entry with a placeholder hash so we can compute the real one.
        let mut entry = AuditEntry {
            id: Uuid::new_v4(),
            sequence,
            timestamp: Utc::now(),
            action,
            actor: actor.to_string(),
            metadata,
            prev_hash,
            hash: String::new(),
        };

        entry.hash = compute_entry_hash(&entry);
        self.entries.push(entry);
        self.next_sequence = sequence + 1;

        // SAFETY: we just pushed, so last() is always Some.
        self.entries.last().expect("just pushed")
    }

    /// Verify the entire hash chain from genesis to the latest entry.
    ///
    /// Returns `Ok(())` if the bout log is intact, or an error describing the
    /// first inconsistency found.
    pub fn verify_chain(&self) -> Result<(), AuditVerifyError> {
        let mut expected_prev_hash = String::new();

        for (expected_sequence, entry) in (0_u64..).zip(self.entries.iter()) {
            // Check sequence continuity.
            if entry.sequence != expected_sequence {
                return Err(AuditVerifyError::SequenceGap {
                    expected: expected_sequence,
                    actual: entry.sequence,
                });
            }

            // Check the chain link.
            if entry.prev_hash != expected_prev_hash {
                return Err(AuditVerifyError::ChainBroken {
                    sequence: entry.sequence,
                    expected_prev: expected_prev_hash,
                    actual_prev: entry.prev_hash.clone(),
                });
            }

            // Recompute and compare the hash.
            let recomputed = compute_entry_hash(entry);
            if entry.hash != recomputed {
                return Err(AuditVerifyError::HashMismatch {
                    sequence: entry.sequence,
                    expected: recomputed,
                    actual: entry.hash.clone(),
                });
            }

            expected_prev_hash = entry.hash.clone();
        }

        Ok(())
    }

    /// Return all entries in the bout log.
    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    /// Return the most recent entry, if any.
    pub fn last_entry(&self) -> Option<&AuditEntry> {
        self.entries.last()
    }

    /// Return all entries with a sequence number strictly greater than `sequence`.
    pub fn entries_since(&self, sequence: u64) -> &[AuditEntry] {
        // Entries are ordered by sequence, so we can binary-search for the
        // first entry whose sequence > the given value.
        let start = self.entries.partition_point(|e| e.sequence <= sequence);
        &self.entries[start..]
    }

    /// Return entries performed by the given `actor`.
    pub fn entries_by_actor(&self, actor: &str) -> Vec<&AuditEntry> {
        self.entries.iter().filter(|e| e.actor == actor).collect()
    }

    /// Return entries matching the given action type name (e.g. `"ToolExecuted"`).
    pub fn entries_by_action_type(&self, action_type: &str) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .filter(|e| e.action.type_name() == action_type)
            .collect()
    }

    /// Number of entries in the bout log.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the bout log is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Helper: create a simple tool-executed action.
    fn tool_executed(tool: &str, fighter: &str, success: bool) -> AuditAction {
        AuditAction::ToolExecuted {
            tool: tool.to_string(),
            fighter_id: fighter.to_string(),
            success,
        }
    }

    #[test]
    fn genesis_entry_has_empty_prev_hash() {
        let mut log = AuditLog::new();
        log.append(tool_executed("ls", "f1", true), "f1", json!({}));

        let genesis = &log.entries()[0];
        assert!(
            genesis.prev_hash.is_empty(),
            "genesis prev_hash must be empty"
        );
        assert!(!genesis.hash.is_empty(), "genesis hash must not be empty");
        assert_eq!(genesis.sequence, 0);
    }

    #[test]
    fn second_entry_prev_hash_matches_first_hash() {
        let mut log = AuditLog::new();
        log.append(tool_executed("ls", "f1", true), "f1", json!({}));
        log.append(tool_executed("cat", "f1", true), "f1", json!({}));

        let first = &log.entries()[0];
        let second = &log.entries()[1];
        assert_eq!(second.prev_hash, first.hash);
    }

    #[test]
    fn chain_of_10_verifies_cleanly() {
        let mut log = AuditLog::new();
        for i in 0..10 {
            log.append(
                tool_executed(&format!("tool_{i}"), "f1", true),
                "f1",
                json!({ "i": i }),
            );
        }
        assert_eq!(log.len(), 10);
        assert!(log.verify_chain().is_ok());
    }

    #[test]
    fn tampered_content_detected() {
        let mut log = AuditLog::new();
        log.append(tool_executed("ls", "f1", true), "f1", json!({}));
        log.append(tool_executed("cat", "f1", true), "f1", json!({}));

        // Tamper with the actor field of the second entry.
        log.entries[1].actor = "evil".to_string();

        let err = log.verify_chain().unwrap_err();
        match err {
            AuditVerifyError::HashMismatch { sequence, .. } => assert_eq!(sequence, 1),
            other => panic!("expected HashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn tampered_hash_detected() {
        let mut log = AuditLog::new();
        log.append(tool_executed("ls", "f1", true), "f1", json!({}));
        log.append(tool_executed("cat", "f1", true), "f1", json!({}));

        // Overwrite the first entry's hash with garbage.
        log.entries[0].hash = "deadbeef".to_string();

        let err = log.verify_chain().unwrap_err();
        // Could be HashMismatch on entry 0 or ChainBroken on entry 1.
        match err {
            AuditVerifyError::HashMismatch { sequence, .. } => assert_eq!(sequence, 0),
            AuditVerifyError::ChainBroken { sequence, .. } => assert_eq!(sequence, 1),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn broken_chain_swap_entries() {
        let mut log = AuditLog::new();
        log.append(tool_executed("a", "f1", true), "f1", json!({}));
        log.append(tool_executed("b", "f1", true), "f1", json!({}));
        log.append(tool_executed("c", "f1", true), "f1", json!({}));

        // Swap entries 1 and 2.
        log.entries.swap(1, 2);

        assert!(log.verify_chain().is_err());
    }

    #[test]
    fn sequence_gap_detected() {
        let mut log = AuditLog::new();
        log.append(tool_executed("a", "f1", true), "f1", json!({}));
        log.append(tool_executed("b", "f1", true), "f1", json!({}));

        // Introduce a gap by bumping the second entry's sequence.
        log.entries[1].sequence = 5;

        let err = log.verify_chain().unwrap_err();
        match err {
            AuditVerifyError::SequenceGap {
                expected, actual, ..
            } => {
                assert_eq!(expected, 1);
                assert_eq!(actual, 5);
            }
            other => panic!("expected SequenceGap, got {other:?}"),
        }
    }

    #[test]
    fn entries_since_returns_correct_subset() {
        let mut log = AuditLog::new();
        for i in 0..5 {
            log.append(tool_executed(&format!("t{i}"), "f1", true), "f1", json!({}));
        }

        let since_2 = log.entries_since(2);
        assert_eq!(since_2.len(), 2); // sequences 3 and 4
        assert_eq!(since_2[0].sequence, 3);
        assert_eq!(since_2[1].sequence, 4);
    }

    #[test]
    fn entries_by_actor_filters_correctly() {
        let mut log = AuditLog::new();
        log.append(tool_executed("a", "f1", true), "f1", json!({}));
        log.append(tool_executed("b", "f2", true), "f2", json!({}));
        log.append(tool_executed("c", "f1", true), "f1", json!({}));

        let f1_entries = log.entries_by_actor("f1");
        assert_eq!(f1_entries.len(), 2);
        assert!(f1_entries.iter().all(|e| e.actor == "f1"));

        let f2_entries = log.entries_by_actor("f2");
        assert_eq!(f2_entries.len(), 1);
    }

    #[test]
    fn entries_by_action_type_filters_correctly() {
        let mut log = AuditLog::new();
        log.append(tool_executed("a", "f1", true), "f1", json!({}));
        log.append(
            AuditAction::ToolBlocked {
                tool: "rm".to_string(),
                fighter_id: "f1".to_string(),
                reason: "dangerous".to_string(),
            },
            "system",
            json!({}),
        );
        log.append(tool_executed("b", "f1", true), "f1", json!({}));

        let executed = log.entries_by_action_type("ToolExecuted");
        assert_eq!(executed.len(), 2);

        let blocked = log.entries_by_action_type("ToolBlocked");
        assert_eq!(blocked.len(), 1);
    }

    #[test]
    fn empty_audit_log_verifies_cleanly() {
        let log = AuditLog::new();
        assert!(log.verify_chain().is_ok());
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
        assert!(log.last_entry().is_none());
    }

    #[test]
    fn last_entry_returns_correct_entry() {
        let mut log = AuditLog::new();
        log.append(tool_executed("first", "f1", true), "f1", json!({}));
        log.append(tool_executed("second", "f1", true), "f1", json!({}));
        log.append(tool_executed("third", "f1", true), "f1", json!({}));

        let last = log.last_entry().unwrap();
        assert_eq!(last.sequence, 2);
        match &last.action {
            AuditAction::ToolExecuted { tool, .. } => assert_eq!(tool, "third"),
            other => panic!("unexpected action: {other:?}"),
        }
    }

    #[test]
    fn serialization_roundtrip_preserves_hashes() {
        let mut log = AuditLog::new();
        log.append(tool_executed("ls", "f1", true), "f1", json!({"key": "val"}));
        log.append(
            AuditAction::FighterSpawned {
                fighter_id: "f2".to_string(),
                name: "challenger".to_string(),
            },
            "system",
            json!({}),
        );

        let serialized = serde_json::to_string(&log).unwrap();
        let deserialized: AuditLog = serde_json::from_str(&serialized).unwrap();

        assert!(deserialized.verify_chain().is_ok());
        assert_eq!(deserialized.len(), log.len());
        for (orig, deser) in log.entries().iter().zip(deserialized.entries().iter()) {
            assert_eq!(orig.hash, deser.hash);
            assert_eq!(orig.prev_hash, deser.prev_hash);
            assert_eq!(orig.sequence, deser.sequence);
        }
    }

    #[test]
    fn multiple_action_types_coexist() {
        let mut log = AuditLog::new();
        log.append(tool_executed("ls", "f1", true), "f1", json!({}));
        log.append(
            AuditAction::ToolBlocked {
                tool: "rm".to_string(),
                fighter_id: "f1".to_string(),
                reason: "forbidden".to_string(),
            },
            "system",
            json!({}),
        );
        log.append(
            AuditAction::ApprovalRequested {
                tool: "deploy".to_string(),
                fighter_id: "f1".to_string(),
                risk_level: "high".to_string(),
            },
            "f1",
            json!({}),
        );
        log.append(
            AuditAction::ApprovalGranted {
                tool: "deploy".to_string(),
                fighter_id: "f1".to_string(),
            },
            "user",
            json!({}),
        );
        log.append(
            AuditAction::CapabilityGranted {
                capability: "file_write".to_string(),
                fighter_id: "f1".to_string(),
                granted_by: "user".to_string(),
            },
            "user",
            json!({}),
        );
        log.append(
            AuditAction::TaintDetected {
                source: "env".to_string(),
                value_preview: "SECRET_K***".to_string(),
                severity: "high".to_string(),
            },
            "system",
            json!({}),
        );
        log.append(
            AuditAction::FighterSpawned {
                fighter_id: "f2".to_string(),
                name: "contender".to_string(),
            },
            "system",
            json!({}),
        );
        log.append(
            AuditAction::SessionStarted {
                bout_id: "bout-1".to_string(),
                fighter_id: "f2".to_string(),
            },
            "system",
            json!({}),
        );
        log.append(
            AuditAction::ConfigChanged {
                key: "max_tokens".to_string(),
                old_preview: "4096".to_string(),
                new_preview: "8192".to_string(),
            },
            "user",
            json!({}),
        );

        assert_eq!(log.len(), 9);
        assert!(log.verify_chain().is_ok());

        // Verify various action type queries return correct counts.
        assert_eq!(log.entries_by_action_type("ToolExecuted").len(), 1);
        assert_eq!(log.entries_by_action_type("ToolBlocked").len(), 1);
        assert_eq!(log.entries_by_action_type("ApprovalRequested").len(), 1);
        assert_eq!(log.entries_by_action_type("ApprovalGranted").len(), 1);
        assert_eq!(log.entries_by_action_type("CapabilityGranted").len(), 1);
        assert_eq!(log.entries_by_action_type("TaintDetected").len(), 1);
        assert_eq!(log.entries_by_action_type("FighterSpawned").len(), 1);
        assert_eq!(log.entries_by_action_type("SessionStarted").len(), 1);
        assert_eq!(log.entries_by_action_type("ConfigChanged").len(), 1);
    }
}
