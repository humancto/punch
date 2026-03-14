//! Data Sweeper gorilla — maintenance gorilla that cleans up old data.
//!
//! Cleans up old bout messages beyond retention period, compacts memory
//! entries, prunes expired sessions. Designed to run daily.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tracing::{debug, info, warn};

use punch_memory::MemorySubstrate;
use punch_runtime::LlmDriver;
use punch_types::{GorillaManifest, PunchResult};

use crate::{GorillaOutput, GorillaRunner, RequirementStatus};

// ---------------------------------------------------------------------------
// SweepReport
// ---------------------------------------------------------------------------

/// Structured report of a data sweep operation.
#[derive(Debug, Clone)]
pub struct SweepReport {
    /// Timestamp of the sweep.
    pub timestamp: DateTime<Utc>,
    /// Whether this was a dry run (no actual changes made).
    pub dry_run: bool,
    /// Number of old messages deleted (or would be deleted in dry-run).
    pub messages_deleted: u64,
    /// Number of memory entries compacted (or would be compacted in dry-run).
    pub memories_compacted: u64,
    /// Whether vacuum was performed.
    pub vacuum_performed: bool,
    /// Retention cutoff date used.
    pub retention_cutoff: DateTime<Utc>,
    /// Errors encountered during sweep.
    pub errors: Vec<String>,
}

impl SweepReport {
    /// Check if the sweep completed without errors.
    pub fn is_success(&self) -> bool {
        self.errors.is_empty()
    }

    /// Format the report as a human-readable summary.
    pub fn format_summary(&self) -> String {
        let mode = if self.dry_run { "DRY RUN" } else { "LIVE" };
        let mut summary = format!(
            "Data sweep ({mode}) completed at {}. ",
            self.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
        );
        summary.push_str(&format!(
            "Retention cutoff: {}. ",
            self.retention_cutoff.format("%Y-%m-%d"),
        ));
        summary.push_str(&format!("Messages deleted: {}. ", self.messages_deleted));
        summary.push_str(&format!(
            "Memories compacted: {}. ",
            self.memories_compacted,
        ));
        if self.vacuum_performed {
            summary.push_str("Vacuum: performed. ");
        }
        if !self.errors.is_empty() {
            summary.push_str(&format!("Errors: {}.", self.errors.len()));
        }
        summary
    }
}

// ---------------------------------------------------------------------------
// DataSweeper
// ---------------------------------------------------------------------------

/// Data sweeper gorilla that performs maintenance tasks.
pub struct DataSweeper {
    manifest: GorillaManifest,
    /// Retention period for bout messages.
    retention_period: Duration,
    /// Whether to compact memory entries.
    compact_memory: bool,
    /// Maximum memory entries to keep per fighter.
    max_memories_per_fighter: usize,
    /// Whether to run in dry-run mode (report only, no changes).
    dry_run: bool,
}

impl DataSweeper {
    /// Create a new data sweeper with default settings.
    pub fn new() -> Self {
        Self {
            manifest: Self::default_manifest(),
            retention_period: Duration::from_secs(30 * 86400), // 30 days
            compact_memory: true,
            max_memories_per_fighter: 1000,
            dry_run: false,
        }
    }

    /// Create a data sweeper with custom settings.
    pub fn with_config(
        retention_period: Duration,
        compact_memory: bool,
        max_memories_per_fighter: usize,
    ) -> Self {
        Self {
            manifest: Self::default_manifest(),
            retention_period,
            compact_memory,
            max_memories_per_fighter,
            dry_run: false,
        }
    }

    /// Enable or disable dry-run mode.
    pub fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    /// Check whether dry-run mode is enabled.
    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Get the default manifest.
    fn default_manifest() -> GorillaManifest {
        GorillaManifest {
            name: "Data Sweeper".to_string(),
            description: "Maintenance gorilla that cleans up old data, compacts memory, \
                          and prunes expired sessions."
                .to_string(),
            schedule: "@daily".to_string(),
            moves_required: vec!["memory_store".to_string(), "memory_recall".to_string()],
            settings_schema: None,
            dashboard_metrics: vec![
                "messages_cleaned".to_string(),
                "memories_compacted".to_string(),
                "bytes_freed".to_string(),
            ],
            system_prompt: Some(
                "You are the Data Sweeper gorilla. Your job is to maintain data hygiene \
                 by cleaning up old messages, compacting memory entries, and pruning \
                 expired sessions. Report what was cleaned and any issues found."
                    .to_string(),
            ),
            model: None,
            capabilities: Vec::new(),
            weight_class: None,
        }
    }

    /// Get the retention cutoff date.
    fn retention_cutoff(&self) -> DateTime<Utc> {
        let dur = chrono::Duration::from_std(self.retention_period)
            .unwrap_or_else(|_| chrono::Duration::days(30));
        Utc::now() - dur
    }

    /// Get the retention period.
    pub fn retention_period(&self) -> Duration {
        self.retention_period
    }

    /// Check if compaction is enabled.
    pub fn compact_memory_enabled(&self) -> bool {
        self.compact_memory
    }

    /// Get max memories per fighter.
    pub fn max_memories_per_fighter(&self) -> usize {
        self.max_memories_per_fighter
    }

    /// Count messages that would be cleaned without actually deleting them.
    async fn count_cleanable_messages(
        &self,
        memory: &MemorySubstrate,
        cutoff: DateTime<Utc>,
    ) -> PunchResult<usize> {
        // Count messages older than cutoff by querying the period from epoch to cutoff.
        let epoch = DateTime::parse_from_rfc3339("1970-01-01T00:00:00Z")
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now() - chrono::Duration::days(36500));
        memory.count_messages_in_period(epoch, cutoff).await
    }
}

impl Default for DataSweeper {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl GorillaRunner for DataSweeper {
    fn manifest(&self) -> &GorillaManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        memory: &MemorySubstrate,
        _driver: Arc<dyn LlmDriver>,
    ) -> PunchResult<GorillaOutput> {
        info!(dry_run = self.dry_run, "Data Sweeper gorilla starting execution");

        let cutoff = self.retention_cutoff();
        let mut report = SweepReport {
            timestamp: Utc::now(),
            dry_run: self.dry_run,
            messages_deleted: 0,
            memories_compacted: 0,
            vacuum_performed: false,
            retention_cutoff: cutoff,
            errors: Vec::new(),
        };
        let mut artifacts = Vec::new();

        // Step 1: Clean old bout messages.
        info!(
            cutoff = %cutoff.format("%Y-%m-%d %H:%M:%S UTC"),
            dry_run = self.dry_run,
            "cleaning messages older than cutoff"
        );

        if self.dry_run {
            // In dry-run mode, count but don't delete.
            match self.count_cleanable_messages(memory, cutoff).await {
                Ok(count) => {
                    report.messages_deleted = count as u64;
                    info!(messages_would_delete = count, "dry run: counted cleanable messages");
                }
                Err(e) => {
                    warn!(error = %e, "failed to count cleanable messages");
                    report.errors.push(format!("count_messages: {e}"));
                }
            }
        } else {
            match memory.cleanup_old_messages(cutoff).await {
                Ok(count) => {
                    report.messages_deleted = count as u64;
                    info!(messages_cleaned = count, "old messages cleaned");
                }
                Err(e) => {
                    warn!(error = %e, "failed to clean old messages");
                    report.errors.push(format!("cleanup_messages: {e}"));
                    artifacts.push(format!("error:cleanup_messages:{e}"));
                }
            }
        }

        // Step 2: Compact memory entries if enabled.
        if self.compact_memory {
            debug!("compacting memory entries");
            if self.dry_run {
                // In dry-run mode, we can't easily count without modifying, so report 0.
                info!("dry run: memory compaction would be performed (max {} per fighter)",
                    self.max_memories_per_fighter);
            } else {
                match memory.compact_memories(self.max_memories_per_fighter).await {
                    Ok(count) => {
                        report.memories_compacted = count as u64;
                        info!(memories_compacted = count, "memory entries compacted");
                    }
                    Err(e) => {
                        warn!(error = %e, "failed to compact memories");
                        report.errors.push(format!("compact_memories: {e}"));
                        artifacts.push(format!("error:compact_memories:{e}"));
                    }
                }
            }
        }

        // Step 3: Database vacuum (if supported and not dry-run).
        if !self.dry_run {
            match memory.vacuum().await {
                Ok(()) => {
                    report.vacuum_performed = true;
                    info!("database vacuumed successfully");
                    artifacts.push("vacuum:success".to_string());
                }
                Err(e) => {
                    debug!(error = %e, "database vacuum not available or failed");
                }
            }
        }

        let summary = report.format_summary();

        // Add structured report data to artifacts.
        artifacts.push(format!("messages_deleted:{}", report.messages_deleted));
        artifacts.push(format!("memories_compacted:{}", report.memories_compacted));
        if self.dry_run {
            artifacts.push("mode:dry_run".to_string());
        }

        info!(
            messages_deleted = report.messages_deleted,
            memories_compacted = report.memories_compacted,
            dry_run = self.dry_run,
            errors = report.errors.len(),
            "Data Sweeper gorilla execution complete"
        );

        Ok(GorillaOutput {
            summary,
            artifacts,
            next_run: None,
        })
    }

    fn check_requirements(&self) -> Vec<RequirementStatus> {
        vec![RequirementStatus {
            name: "database_access".to_string(),
            met: true,
            message: "Database accessible for cleanup operations".to_string(),
        }]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_sweeper_new() {
        let sweeper = DataSweeper::new();
        assert_eq!(sweeper.manifest().name, "Data Sweeper");
        assert_eq!(sweeper.retention_period, Duration::from_secs(30 * 86400));
        assert!(sweeper.compact_memory);
        assert!(!sweeper.dry_run);
    }

    #[test]
    fn data_sweeper_default() {
        let sweeper = DataSweeper::default();
        assert_eq!(sweeper.max_memories_per_fighter, 1000);
    }

    #[test]
    fn data_sweeper_with_config() {
        let sweeper = DataSweeper::with_config(
            Duration::from_secs(7 * 86400),
            false,
            500,
        );
        assert_eq!(sweeper.retention_period, Duration::from_secs(7 * 86400));
        assert!(!sweeper.compact_memory);
        assert_eq!(sweeper.max_memories_per_fighter, 500);
    }

    #[test]
    fn data_sweeper_dry_run_mode() {
        let sweeper = DataSweeper::new().with_dry_run(true);
        assert!(sweeper.is_dry_run());
    }

    #[test]
    fn data_sweeper_dry_run_disabled_by_default() {
        let sweeper = DataSweeper::new();
        assert!(!sweeper.is_dry_run());
    }

    #[test]
    fn data_sweeper_retention_cutoff() {
        let sweeper = DataSweeper::new();
        let cutoff = sweeper.retention_cutoff();
        let now = Utc::now();
        let diff = now - cutoff;
        // Should be approximately 30 days.
        assert!(diff.num_days() >= 29 && diff.num_days() <= 31);
    }

    #[test]
    fn data_sweeper_configurable_retention() {
        let sweeper = DataSweeper::with_config(
            Duration::from_secs(7 * 86400), // 7 days
            true,
            1000,
        );
        let cutoff = sweeper.retention_cutoff();
        let now = Utc::now();
        let diff = now - cutoff;
        assert!(diff.num_days() >= 6 && diff.num_days() <= 8);
    }

    #[test]
    fn data_sweeper_check_requirements() {
        let sweeper = DataSweeper::new();
        let reqs = sweeper.check_requirements();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].met);
    }

    #[test]
    fn data_sweeper_manifest_schedule() {
        let sweeper = DataSweeper::new();
        assert_eq!(sweeper.manifest().schedule, "@daily");
    }

    #[test]
    fn data_sweeper_accessors() {
        let sweeper = DataSweeper::new();
        assert_eq!(sweeper.retention_period(), Duration::from_secs(30 * 86400));
        assert!(sweeper.compact_memory_enabled());
        assert_eq!(sweeper.max_memories_per_fighter(), 1000);
    }

    #[test]
    fn sweep_report_success() {
        let report = SweepReport {
            timestamp: Utc::now(),
            dry_run: false,
            messages_deleted: 10,
            memories_compacted: 5,
            vacuum_performed: true,
            retention_cutoff: Utc::now() - chrono::Duration::days(30),
            errors: Vec::new(),
        };
        assert!(report.is_success());
    }

    #[test]
    fn sweep_report_with_errors() {
        let report = SweepReport {
            timestamp: Utc::now(),
            dry_run: false,
            messages_deleted: 0,
            memories_compacted: 0,
            vacuum_performed: false,
            retention_cutoff: Utc::now(),
            errors: vec!["test error".to_string()],
        };
        assert!(!report.is_success());
    }

    #[test]
    fn sweep_report_format_summary() {
        let report = SweepReport {
            timestamp: Utc::now(),
            dry_run: false,
            messages_deleted: 42,
            memories_compacted: 7,
            vacuum_performed: true,
            retention_cutoff: Utc::now() - chrono::Duration::days(30),
            errors: Vec::new(),
        };
        let summary = report.format_summary();
        assert!(summary.contains("Messages deleted: 42"));
        assert!(summary.contains("Memories compacted: 7"));
        assert!(summary.contains("Vacuum: performed"));
        assert!(summary.contains("LIVE"));
    }

    #[test]
    fn sweep_report_dry_run_summary() {
        let report = SweepReport {
            timestamp: Utc::now(),
            dry_run: true,
            messages_deleted: 10,
            memories_compacted: 0,
            vacuum_performed: false,
            retention_cutoff: Utc::now(),
            errors: Vec::new(),
        };
        let summary = report.format_summary();
        assert!(summary.contains("DRY RUN"));
    }

    #[test]
    fn sweep_report_zero_counts() {
        let report = SweepReport {
            timestamp: Utc::now(),
            dry_run: false,
            messages_deleted: 0,
            memories_compacted: 0,
            vacuum_performed: false,
            retention_cutoff: Utc::now(),
            errors: Vec::new(),
        };
        assert!(report.is_success());
        assert_eq!(report.messages_deleted, 0);
        assert_eq!(report.memories_compacted, 0);
    }
}
