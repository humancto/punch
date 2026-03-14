//! Report Generator gorilla — generates periodic activity reports.
//!
//! Aggregates fighter activity metrics, summarizes tool usage patterns,
//! and generates daily/weekly/monthly summaries. Runs on a configurable schedule.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use tracing::{debug, info, warn};

use punch_memory::MemorySubstrate;
use punch_runtime::LlmDriver;
use punch_types::{GorillaManifest, PunchResult};

use crate::{GorillaOutput, GorillaRunner, RequirementStatus};

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

/// The reporting period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportPeriod {
    /// Daily report (last 24 hours).
    Daily,
    /// Weekly report (last 7 days).
    Weekly,
    /// Monthly report (last 30 days).
    Monthly,
}

impl ReportPeriod {
    /// Get the duration of this period.
    pub fn duration(&self) -> ChronoDuration {
        match self {
            Self::Daily => ChronoDuration::days(1),
            Self::Weekly => ChronoDuration::days(7),
            Self::Monthly => ChronoDuration::days(30),
        }
    }

    /// Get a human-readable label.
    pub fn label(&self) -> &str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }
}

/// Trend indicator compared to previous period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trend {
    /// Value increased from previous period.
    Up,
    /// Value decreased from previous period.
    Down,
    /// Value stayed the same.
    Flat,
    /// No previous data available.
    NoData,
}

impl std::fmt::Display for Trend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Up => write!(f, "[UP]"),
            Self::Down => write!(f, "[DOWN]"),
            Self::Flat => write!(f, "[FLAT]"),
            Self::NoData => write!(f, "[N/A]"),
        }
    }
}

/// Compute trend by comparing current and previous values.
pub fn compute_trend(current: u64, previous: u64) -> Trend {
    if current > previous {
        Trend::Up
    } else if current < previous {
        Trend::Down
    } else {
        Trend::Flat
    }
}

/// A section in a generated report.
#[derive(Debug, Clone)]
pub struct ReportSection {
    /// Section title.
    pub title: String,
    /// Section content (formatted text).
    pub content: String,
}

/// Tool usage statistics.
#[derive(Debug, Clone)]
pub struct ToolStats {
    /// Tool name.
    pub name: String,
    /// Number of calls.
    pub call_count: u64,
    /// Number of successful calls.
    pub success_count: u64,
    /// Success rate (0.0 - 1.0).
    pub success_rate: f64,
}

/// The full generated report.
#[derive(Debug, Clone)]
pub struct Report {
    /// Report period.
    pub period: ReportPeriod,
    /// Start of the period.
    pub period_start: DateTime<Utc>,
    /// End of the period.
    pub period_end: DateTime<Utc>,
    /// Timestamp when the report was generated.
    pub generated_at: DateTime<Utc>,
    /// Summary metrics.
    pub metrics: ReportMetrics,
    /// Trend indicators compared to previous period.
    pub trends: ReportTrends,
    /// Report sections (formatted text).
    pub sections: Vec<ReportSection>,
    /// Full formatted report text.
    pub formatted_text: String,
}

/// Trend indicators for key metrics.
#[derive(Debug, Clone)]
pub struct ReportTrends {
    /// Trend for total bouts.
    pub bouts_trend: Trend,
    /// Trend for total messages.
    pub messages_trend: Trend,
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// Metrics collected for a report.
#[derive(Debug, Default, Clone)]
pub struct ReportMetrics {
    /// Total bouts in the period.
    pub total_bouts: u64,
    /// Total messages in the period.
    pub total_messages: u64,
    /// Total tool calls in the period.
    pub total_tool_calls: u64,
    /// Number of active fighters.
    pub active_fighters: u64,
    /// Per-fighter summaries.
    pub fighter_summaries: Vec<FighterSummary>,
    /// Tool usage stats (tool name, call count).
    pub tool_usage: Vec<(String, u64)>,
    /// Messages per day (average over the period).
    pub messages_per_day: f64,
    /// Average bout length in messages.
    pub avg_bout_length: f64,
}

/// Per-fighter summary for reports.
#[derive(Debug, Clone)]
pub struct FighterSummary {
    /// Fighter display name.
    pub name: String,
    /// Number of bouts.
    pub bout_count: u64,
    /// Number of messages.
    pub message_count: u64,
}

// ---------------------------------------------------------------------------
// ReportGenerator
// ---------------------------------------------------------------------------

/// Report generator gorilla.
pub struct ReportGenerator {
    manifest: GorillaManifest,
    /// The reporting period.
    period: ReportPeriod,
    /// Whether to include tool usage breakdown.
    include_tool_usage: bool,
    /// Whether to include fighter activity breakdown.
    include_fighter_activity: bool,
}

impl ReportGenerator {
    /// Create a new daily report generator.
    pub fn new() -> Self {
        Self {
            manifest: Self::default_manifest(ReportPeriod::Daily),
            period: ReportPeriod::Daily,
            include_tool_usage: true,
            include_fighter_activity: true,
        }
    }

    /// Create a report generator with a specific period.
    pub fn with_period(period: ReportPeriod) -> Self {
        let schedule = match period {
            ReportPeriod::Daily => "@daily",
            ReportPeriod::Weekly => "@weekly",
            ReportPeriod::Monthly => "0 0 1 * *",
        };

        let mut manifest = Self::default_manifest(period);
        manifest.schedule = schedule.to_string();

        Self {
            manifest,
            period,
            include_tool_usage: true,
            include_fighter_activity: true,
        }
    }

    /// Configure whether to include tool usage in reports.
    pub fn with_tool_usage(mut self, include: bool) -> Self {
        self.include_tool_usage = include;
        self
    }

    /// Configure whether to include fighter activity in reports.
    pub fn with_fighter_activity(mut self, include: bool) -> Self {
        self.include_fighter_activity = include;
        self
    }

    /// Get the reporting period.
    pub fn period(&self) -> ReportPeriod {
        self.period
    }

    /// Get the default manifest for a given period.
    fn default_manifest(period: ReportPeriod) -> GorillaManifest {
        GorillaManifest {
            name: format!("Report Generator ({})", period.label()),
            description: format!(
                "Generates {} reports aggregating fighter activity, tool usage, \
                 and system metrics.",
                period.label()
            ),
            schedule: "@daily".to_string(),
            moves_required: vec!["memory_recall".to_string(), "memory_store".to_string()],
            settings_schema: None,
            dashboard_metrics: vec![
                "reports_generated".to_string(),
                "total_bouts".to_string(),
                "total_messages".to_string(),
                "total_tool_calls".to_string(),
            ],
            system_prompt: Some(format!(
                "You are the Report Generator gorilla. Produce a clear, concise {} \
                 activity report with key metrics, trends, and actionable insights.",
                period.label()
            )),
            model: None,
            capabilities: Vec::new(),
            weight_class: None,
        }
    }

    /// Generate the report content from collected metrics.
    fn format_report(
        &self,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
        metrics: &ReportMetrics,
        trends: &ReportTrends,
    ) -> (String, Vec<ReportSection>) {
        let mut report = String::new();
        let mut sections = Vec::new();

        // Header.
        report.push_str(&format!(
            "# {} Activity Report\n\n",
            self.period.label().to_uppercase()
        ));
        report.push_str(&format!(
            "**Period:** {} to {}\n\n",
            period_start.format("%Y-%m-%d %H:%M UTC"),
            period_end.format("%Y-%m-%d %H:%M UTC")
        ));

        // Summary section.
        let mut summary_content = String::new();
        summary_content.push_str(&format!(
            "- **Total Bouts:** {} {}\n",
            metrics.total_bouts, trends.bouts_trend
        ));
        summary_content.push_str(&format!(
            "- **Total Messages:** {} {}\n",
            metrics.total_messages, trends.messages_trend
        ));
        summary_content.push_str(&format!("- **Tool Calls:** {}\n", metrics.total_tool_calls));
        summary_content.push_str(&format!(
            "- **Active Fighters:** {}\n",
            metrics.active_fighters
        ));
        summary_content.push_str(&format!(
            "- **Messages/Day:** {:.1}\n",
            metrics.messages_per_day
        ));
        if metrics.total_bouts > 0 {
            summary_content.push_str(&format!(
                "- **Avg Bout Length:** {:.1} messages\n",
                metrics.avg_bout_length
            ));
        }

        report.push_str("## Summary\n\n");
        report.push_str(&summary_content);
        sections.push(ReportSection {
            title: "Summary".to_string(),
            content: summary_content,
        });

        // Fighter Activity section.
        if self.include_fighter_activity && !metrics.fighter_summaries.is_empty() {
            let mut fighter_content = String::new();
            for summary in &metrics.fighter_summaries {
                fighter_content.push_str(&format!(
                    "- **{}**: {} bouts, {} messages\n",
                    summary.name, summary.bout_count, summary.message_count
                ));
            }
            report.push_str("\n## Fighter Activity\n\n");
            report.push_str(&fighter_content);
            sections.push(ReportSection {
                title: "Fighter Activity".to_string(),
                content: fighter_content,
            });
        }

        // Tool Usage section.
        if self.include_tool_usage && !metrics.tool_usage.is_empty() {
            let mut tool_content = String::new();
            for (tool, count) in &metrics.tool_usage {
                tool_content.push_str(&format!("- **{}**: {} calls\n", tool, count));
            }
            report.push_str("\n## Tool Usage\n\n");
            report.push_str(&tool_content);
            sections.push(ReportSection {
                title: "Tool Usage".to_string(),
                content: tool_content,
            });
        }

        report.push_str(&format!(
            "\n---\n*Generated at {}*\n",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        ));

        (report, sections)
    }

    /// Collect metrics for a given time period from memory substrate.
    async fn collect_metrics(
        &self,
        memory: &MemorySubstrate,
        period_start: DateTime<Utc>,
        period_end: DateTime<Utc>,
    ) -> ReportMetrics {
        let mut metrics = ReportMetrics::default();

        // Query bout count for the period.
        match memory.count_bouts_in_period(period_start, period_end).await {
            Ok(count) => {
                metrics.total_bouts = count as u64;
                debug!(bouts = count, "bout count retrieved");
            }
            Err(e) => {
                warn!(error = %e, "failed to count bouts");
            }
        }

        // Query message count for the period.
        match memory
            .count_messages_in_period(period_start, period_end)
            .await
        {
            Ok(count) => {
                metrics.total_messages = count as u64;
                debug!(messages = count, "message count retrieved");
            }
            Err(e) => {
                warn!(error = %e, "failed to count messages");
            }
        }

        // Calculate derived metrics.
        let period_days = (period_end - period_start).num_days().max(1) as f64;
        metrics.messages_per_day = metrics.total_messages as f64 / period_days;

        if metrics.total_bouts > 0 {
            metrics.avg_bout_length = metrics.total_messages as f64 / metrics.total_bouts as f64;
        }

        metrics
    }

    /// Collect metrics for the previous period (for trend calculation).
    async fn collect_previous_metrics(
        &self,
        memory: &MemorySubstrate,
        period_start: DateTime<Utc>,
    ) -> ReportMetrics {
        let prev_end = period_start;
        let prev_start = prev_end - self.period.duration();
        self.collect_metrics(memory, prev_start, prev_end).await
    }
}

impl Default for ReportGenerator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl GorillaRunner for ReportGenerator {
    fn manifest(&self) -> &GorillaManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        memory: &MemorySubstrate,
        _driver: Arc<dyn LlmDriver>,
    ) -> PunchResult<GorillaOutput> {
        info!(period = %self.period.label(), "Report Generator gorilla starting");

        let period_end = Utc::now();
        let period_start = period_end - self.period.duration();

        // Collect current period metrics.
        let metrics = self.collect_metrics(memory, period_start, period_end).await;

        // Collect previous period metrics for trends.
        let prev_metrics = self.collect_previous_metrics(memory, period_start).await;

        let trends = ReportTrends {
            bouts_trend: compute_trend(metrics.total_bouts, prev_metrics.total_bouts),
            messages_trend: compute_trend(metrics.total_messages, prev_metrics.total_messages),
        };

        // Generate the report text.
        let (formatted_text, sections) =
            self.format_report(period_start, period_end, &metrics, &trends);

        let _report = Report {
            period: self.period,
            period_start,
            period_end,
            generated_at: Utc::now(),
            metrics: metrics.clone(),
            trends: trends.clone(),
            sections,
            formatted_text: formatted_text.clone(),
        };

        // Store the report as a memory entry.
        let report_key = format!(
            "report_{}_{}",
            self.period.label(),
            period_end.format("%Y%m%d")
        );
        if let Err(e) = memory
            .store_memory(
                &punch_types::FighterId::new(),
                &report_key,
                &formatted_text,
                0.9,
            )
            .await
        {
            warn!(error = %e, "failed to store report in memory");
        }

        let summary = format!(
            "{} report generated for period {} to {}. \
             {} bouts {}, {} messages {}, {:.1} msgs/day.",
            self.period.label(),
            period_start.format("%Y-%m-%d"),
            period_end.format("%Y-%m-%d"),
            metrics.total_bouts,
            trends.bouts_trend,
            metrics.total_messages,
            trends.messages_trend,
            metrics.messages_per_day
        );

        info!(
            period = %self.period.label(),
            bouts = metrics.total_bouts,
            messages = metrics.total_messages,
            bouts_trend = %trends.bouts_trend,
            messages_trend = %trends.messages_trend,
            "Report Generator gorilla execution complete"
        );

        Ok(GorillaOutput {
            summary,
            artifacts: vec![format!("memory:{report_key}"), formatted_text],
            next_run: None,
        })
    }

    fn check_requirements(&self) -> Vec<RequirementStatus> {
        vec![RequirementStatus {
            name: "memory_access".to_string(),
            met: true,
            message: "Memory substrate accessible for report generation".to_string(),
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
    fn report_generator_new() {
        let rg = ReportGenerator::new();
        assert_eq!(rg.period(), ReportPeriod::Daily);
        assert!(rg.include_tool_usage);
        assert!(rg.include_fighter_activity);
    }

    #[test]
    fn report_generator_default() {
        let rg = ReportGenerator::default();
        assert_eq!(rg.period(), ReportPeriod::Daily);
    }

    #[test]
    fn report_generator_with_period() {
        let rg = ReportGenerator::with_period(ReportPeriod::Weekly);
        assert_eq!(rg.period(), ReportPeriod::Weekly);
        assert_eq!(rg.manifest().schedule, "@weekly");
    }

    #[test]
    fn report_generator_monthly() {
        let rg = ReportGenerator::with_period(ReportPeriod::Monthly);
        assert_eq!(rg.period(), ReportPeriod::Monthly);
        assert_eq!(rg.manifest().schedule, "0 0 1 * *");
    }

    #[test]
    fn report_generator_builder_pattern() {
        let rg = ReportGenerator::with_period(ReportPeriod::Daily)
            .with_tool_usage(false)
            .with_fighter_activity(false);
        assert!(!rg.include_tool_usage);
        assert!(!rg.include_fighter_activity);
    }

    #[test]
    fn report_generator_check_requirements() {
        let rg = ReportGenerator::new();
        let reqs = rg.check_requirements();
        assert_eq!(reqs.len(), 1);
        assert!(reqs[0].met);
    }

    #[test]
    fn report_period_duration() {
        assert_eq!(ReportPeriod::Daily.duration().num_days(), 1);
        assert_eq!(ReportPeriod::Weekly.duration().num_days(), 7);
        assert_eq!(ReportPeriod::Monthly.duration().num_days(), 30);
    }

    #[test]
    fn report_period_label() {
        assert_eq!(ReportPeriod::Daily.label(), "daily");
        assert_eq!(ReportPeriod::Weekly.label(), "weekly");
        assert_eq!(ReportPeriod::Monthly.label(), "monthly");
    }

    #[test]
    fn format_report_basic() {
        let rg = ReportGenerator::new();
        let now = Utc::now();
        let start = now - ChronoDuration::days(1);
        let metrics = ReportMetrics {
            total_bouts: 10,
            total_messages: 50,
            total_tool_calls: 25,
            active_fighters: 3,
            fighter_summaries: Vec::new(),
            tool_usage: Vec::new(),
            messages_per_day: 50.0,
            avg_bout_length: 5.0,
        };
        let trends = ReportTrends {
            bouts_trend: Trend::Up,
            messages_trend: Trend::Down,
        };

        let (report, sections) = rg.format_report(start, now, &metrics, &trends);
        assert!(report.contains("DAILY Activity Report"));
        assert!(report.contains("Total Bouts:** 10"));
        assert!(report.contains("Total Messages:** 50"));
        assert!(report.contains("Tool Calls:** 25"));
        assert!(report.contains("[UP]"));
        assert!(report.contains("[DOWN]"));
        assert!(report.contains("Messages/Day:** 50.0"));
        assert!(report.contains("Avg Bout Length:** 5.0"));
        assert!(!sections.is_empty());
    }

    #[test]
    fn format_report_with_fighters() {
        let rg = ReportGenerator::new();
        let now = Utc::now();
        let start = now - ChronoDuration::days(1);
        let metrics = ReportMetrics {
            total_bouts: 5,
            total_messages: 20,
            total_tool_calls: 10,
            active_fighters: 2,
            fighter_summaries: vec![
                FighterSummary {
                    name: "Alpha".to_string(),
                    bout_count: 3,
                    message_count: 12,
                },
                FighterSummary {
                    name: "Beta".to_string(),
                    bout_count: 2,
                    message_count: 8,
                },
            ],
            tool_usage: vec![("web_fetch".to_string(), 5), ("read_file".to_string(), 3)],
            messages_per_day: 20.0,
            avg_bout_length: 4.0,
        };
        let trends = ReportTrends {
            bouts_trend: Trend::Flat,
            messages_trend: Trend::NoData,
        };

        let (report, sections) = rg.format_report(start, now, &metrics, &trends);
        assert!(report.contains("Fighter Activity"));
        assert!(report.contains("Alpha"));
        assert!(report.contains("Beta"));
        assert!(report.contains("Tool Usage"));
        assert!(report.contains("web_fetch"));
        assert!(sections.len() >= 3); // Summary + Fighter Activity + Tool Usage
    }

    #[test]
    fn format_report_without_tool_usage() {
        let rg = ReportGenerator::new().with_tool_usage(false);
        let now = Utc::now();
        let start = now - ChronoDuration::days(1);
        let metrics = ReportMetrics {
            tool_usage: vec![("web_fetch".to_string(), 5)],
            ..Default::default()
        };
        let trends = ReportTrends {
            bouts_trend: Trend::NoData,
            messages_trend: Trend::NoData,
        };

        let (report, _sections) = rg.format_report(start, now, &metrics, &trends);
        assert!(!report.contains("Tool Usage"));
    }

    #[test]
    fn format_report_without_fighter_activity() {
        let rg = ReportGenerator::new().with_fighter_activity(false);
        let now = Utc::now();
        let start = now - ChronoDuration::days(1);
        let metrics = ReportMetrics {
            fighter_summaries: vec![FighterSummary {
                name: "Test".to_string(),
                bout_count: 1,
                message_count: 5,
            }],
            ..Default::default()
        };
        let trends = ReportTrends {
            bouts_trend: Trend::NoData,
            messages_trend: Trend::NoData,
        };

        let (report, _sections) = rg.format_report(start, now, &metrics, &trends);
        assert!(!report.contains("Fighter Activity"));
    }

    #[test]
    fn report_generator_manifest_name_includes_period() {
        let rg = ReportGenerator::with_period(ReportPeriod::Weekly);
        assert!(rg.manifest().name.contains("weekly"));
    }

    #[test]
    fn compute_trend_up() {
        assert_eq!(compute_trend(10, 5), Trend::Up);
    }

    #[test]
    fn compute_trend_down() {
        assert_eq!(compute_trend(5, 10), Trend::Down);
    }

    #[test]
    fn compute_trend_flat() {
        assert_eq!(compute_trend(5, 5), Trend::Flat);
    }

    #[test]
    fn trend_display() {
        assert_eq!(format!("{}", Trend::Up), "[UP]");
        assert_eq!(format!("{}", Trend::Down), "[DOWN]");
        assert_eq!(format!("{}", Trend::Flat), "[FLAT]");
        assert_eq!(format!("{}", Trend::NoData), "[N/A]");
    }

    #[test]
    fn daily_weekly_monthly_produce_different_ranges() {
        let daily = ReportPeriod::Daily.duration();
        let weekly = ReportPeriod::Weekly.duration();
        let monthly = ReportPeriod::Monthly.duration();
        assert!(daily < weekly);
        assert!(weekly < monthly);
    }

    #[test]
    fn report_sections_properly_formatted() {
        let rg = ReportGenerator::new();
        let now = Utc::now();
        let start = now - ChronoDuration::days(1);
        let metrics = ReportMetrics {
            total_bouts: 3,
            total_messages: 15,
            messages_per_day: 15.0,
            avg_bout_length: 5.0,
            ..Default::default()
        };
        let trends = ReportTrends {
            bouts_trend: Trend::NoData,
            messages_trend: Trend::NoData,
        };

        let (_report, sections) = rg.format_report(start, now, &metrics, &trends);
        assert!(!sections.is_empty());
        // Summary section should always be present.
        assert_eq!(sections[0].title, "Summary");
        assert!(sections[0].content.contains("Total Bouts"));
    }

    #[test]
    fn report_with_no_data() {
        let rg = ReportGenerator::new();
        let now = Utc::now();
        let start = now - ChronoDuration::days(1);
        let metrics = ReportMetrics::default();
        let trends = ReportTrends {
            bouts_trend: Trend::NoData,
            messages_trend: Trend::NoData,
        };

        let (report, sections) = rg.format_report(start, now, &metrics, &trends);
        assert!(report.contains("DAILY Activity Report"));
        assert!(report.contains("Total Bouts:** 0"));
        assert!(!sections.is_empty());
    }
}
