//! Gorilla Scheduler — cron-based scheduling for autonomous gorilla execution.
//!
//! Supports cron expressions, interval shortcuts (`@every 5m`, `@hourly`, `@daily`),
//! and standard 5-field cron syntax (`0 */6 * * *`). Manages a priority queue of
//! next-run times and handles missed runs with configurable catch-up policy.

use std::collections::BinaryHeap;
use std::cmp::Ordering;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Datelike, NaiveDateTime, Timelike, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, RwLock};
use tracing::{debug, info, warn};

use punch_types::{GorillaId, PunchError, PunchResult};

// ---------------------------------------------------------------------------
// Cron expression parser
// ---------------------------------------------------------------------------

/// A parsed cron expression with 5 fields: minute, hour, day-of-month, month, day-of-week.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronExpression {
    /// Allowed minutes (0-59).
    pub minutes: Vec<u8>,
    /// Allowed hours (0-23).
    pub hours: Vec<u8>,
    /// Allowed days of month (1-31).
    pub days_of_month: Vec<u8>,
    /// Allowed months (1-12).
    pub months: Vec<u8>,
    /// Allowed days of week (0-6, 0=Sunday).
    pub days_of_week: Vec<u8>,
}

impl CronExpression {
    /// Parse a 5-field cron expression string.
    ///
    /// Supports `*`, specific values, ranges (`1-5`), steps (`*/5`), and lists (`1,3,5`).
    pub fn parse(expr: &str) -> PunchResult<Self> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(PunchError::Gorilla(format!(
                "cron expression must have 5 fields, got {}: '{}'",
                fields.len(),
                expr
            )));
        }

        let minutes = parse_cron_field(fields[0], 0, 59)?;
        let hours = parse_cron_field(fields[1], 0, 23)?;
        let days_of_month = parse_cron_field(fields[2], 1, 31)?;
        let months = parse_cron_field(fields[3], 1, 12)?;
        let days_of_week = parse_cron_field(fields[4], 0, 6)?;

        Ok(Self {
            minutes,
            hours,
            days_of_month,
            months,
            days_of_week,
        })
    }

    /// Calculate the next run time after `after`.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        // Start from the next minute after `after`.
        let mut dt = after
            .with_nanosecond(0)?;
        // Advance by one minute to avoid matching the current time exactly.
        dt += chrono::Duration::minutes(1);

        // Search up to 4 years ahead (to handle complex cron patterns).
        let limit = after + chrono::Duration::days(366 * 4);

        while dt < limit {
            let month = dt.month() as u8;
            let day = dt.day() as u8;
            let weekday = dt.weekday().num_days_from_sunday() as u8;
            let hour = dt.hour() as u8;
            let minute = dt.minute() as u8;

            if !self.months.contains(&month) {
                // Skip to next month.
                dt = advance_month(dt)?;
                continue;
            }

            if !self.days_of_month.contains(&day) || !self.days_of_week.contains(&weekday) {
                // Skip to next day.
                dt = advance_day(dt)?;
                continue;
            }

            if !self.hours.contains(&hour) {
                // Skip to next hour.
                dt = advance_hour(dt)?;
                continue;
            }

            if !self.minutes.contains(&minute) {
                dt += chrono::Duration::minutes(1);
                continue;
            }

            return Some(dt);
        }

        None
    }
}

/// Advance to the start of the next month.
fn advance_month(dt: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let (year, month) = if dt.month() == 12 {
        (dt.year() + 1, 1)
    } else {
        (dt.year(), dt.month() + 1)
    };
    NaiveDateTime::new(
        chrono::NaiveDate::from_ymd_opt(year, month, 1)?,
        chrono::NaiveTime::from_hms_opt(0, 0, 0)?,
    )
    .and_utc()
    .into()
}

/// Advance to the start of the next day.
fn advance_day(dt: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let next = dt.date_naive().succ_opt()?;
    NaiveDateTime::new(next, chrono::NaiveTime::from_hms_opt(0, 0, 0)?).and_utc().into()
}

/// Advance to the start of the next hour.
fn advance_hour(dt: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let next = dt.with_minute(0)?.with_second(0)?.with_nanosecond(0)?;
    Some(next + chrono::Duration::hours(1))
}

/// Parse a single cron field (e.g., `*/5`, `1-3`, `1,2,3`, `*`).
fn parse_cron_field(field: &str, min: u8, max: u8) -> PunchResult<Vec<u8>> {
    let mut values = Vec::new();

    for part in field.split(',') {
        let part = part.trim();
        if part == "*" {
            return Ok((min..=max).collect());
        }

        if let Some(step_str) = part.strip_prefix("*/") {
            let step: u8 = step_str
                .parse()
                .map_err(|_| PunchError::Gorilla(format!("invalid cron step: {}", part)))?;
            if step == 0 {
                return Err(PunchError::Gorilla("cron step cannot be 0".to_string()));
            }
            let mut v = min;
            while v <= max {
                values.push(v);
                v = v.saturating_add(step);
            }
        } else if part.contains('-') {
            let parts: Vec<&str> = part.split('-').collect();
            if parts.len() != 2 {
                return Err(PunchError::Gorilla(format!("invalid cron range: {}", part)));
            }
            let start: u8 = parts[0]
                .parse()
                .map_err(|_| PunchError::Gorilla(format!("invalid range start: {}", parts[0])))?;
            let end: u8 = parts[1]
                .parse()
                .map_err(|_| PunchError::Gorilla(format!("invalid range end: {}", parts[1])))?;
            if start > end || start < min || end > max {
                return Err(PunchError::Gorilla(format!(
                    "cron range out of bounds: {}-{} (allowed: {}-{})",
                    start, end, min, max
                )));
            }
            values.extend(start..=end);
        } else {
            let val: u8 = part
                .parse()
                .map_err(|_| PunchError::Gorilla(format!("invalid cron value: {}", part)))?;
            if val < min || val > max {
                return Err(PunchError::Gorilla(format!(
                    "cron value {} out of range {}-{}",
                    val, min, max
                )));
            }
            values.push(val);
        }
    }

    values.sort_unstable();
    values.dedup();
    Ok(values)
}

// ---------------------------------------------------------------------------
// Schedule types
// ---------------------------------------------------------------------------

/// The type of schedule for a gorilla.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Schedule {
    /// Cron-based schedule.
    Cron(CronExpression),
    /// Fixed interval schedule.
    Interval(Duration),
    /// Run once at a specific time.
    OneShot(DateTime<Utc>),
}

/// Policy for handling missed runs (when the scheduler was not running during
/// a scheduled time).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MissedRunPolicy {
    /// Run all missed executions on catch-up.
    CatchUp,
    /// Skip missed runs and schedule the next future run.
    #[default]
    Skip,
    /// Run once to catch up, then resume normal schedule.
    RunOnce,
}

/// Parse a schedule string from a gorilla manifest into a `Schedule`.
///
/// Supports:
/// - `@every 5m`, `@every 30s`, `@every 1h`, `@every 1d`
/// - `@hourly`, `@daily`, `@weekly`
/// - Standard 5-field cron: `0 */6 * * *`, `*/5 * * * *`
/// - Simple interval: `every 5m`
pub fn parse_schedule(schedule: &str) -> PunchResult<Schedule> {
    let s = schedule.trim();

    // Handle shortcut aliases.
    match s.to_lowercase().as_str() {
        "@hourly" => return Ok(Schedule::Interval(Duration::from_secs(3600))),
        "@daily" => return Ok(Schedule::Interval(Duration::from_secs(86400))),
        "@weekly" => return Ok(Schedule::Interval(Duration::from_secs(604800))),
        _ => {}
    }

    // Handle @every or every prefix.
    let interval_str = s
        .strip_prefix("@every ")
        .or_else(|| s.strip_prefix("every "));
    if let Some(interval_str) = interval_str
        && let Some(dur) = parse_duration_str(interval_str.trim())
    {
        return Ok(Schedule::Interval(dur));
    }

    // Try to parse as cron expression.
    let fields: Vec<&str> = s.split_whitespace().collect();
    if fields.len() == 5 {
        let cron = CronExpression::parse(s)?;
        return Ok(Schedule::Cron(cron));
    }

    // Try as a raw interval like "5m", "30s".
    if let Some(dur) = parse_duration_str(s) {
        return Ok(Schedule::Interval(dur));
    }

    Err(PunchError::Gorilla(format!(
        "cannot parse schedule: '{}'",
        schedule
    )))
}

/// Parse a duration string like `5m`, `30s`, `2h`, `1d`.
fn parse_duration_str(s: &str) -> Option<Duration> {
    let s = s.trim().to_lowercase();
    if let Some(num) = s.strip_suffix('s') {
        num.trim().parse::<u64>().ok().map(Duration::from_secs)
    } else if let Some(num) = s.strip_suffix('m') {
        num.trim()
            .parse::<u64>()
            .ok()
            .map(|m| Duration::from_secs(m * 60))
    } else if let Some(num) = s.strip_suffix('h') {
        num.trim()
            .parse::<u64>()
            .ok()
            .map(|h| Duration::from_secs(h * 3600))
    } else if let Some(num) = s.strip_suffix('d') {
        num.trim()
            .parse::<u64>()
            .ok()
            .map(|d| Duration::from_secs(d * 86400))
    } else {
        s.parse::<u64>().ok().map(Duration::from_secs)
    }
}

// ---------------------------------------------------------------------------
// Scheduled entry for the priority queue
// ---------------------------------------------------------------------------

/// An entry in the scheduler's priority queue.
#[derive(Debug, Clone)]
struct ScheduledEntry {
    gorilla_id: GorillaId,
    next_run: DateTime<Utc>,
}

impl PartialEq for ScheduledEntry {
    fn eq(&self, other: &Self) -> bool {
        self.next_run == other.next_run
    }
}

impl Eq for ScheduledEntry {}

impl PartialOrd for ScheduledEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// BinaryHeap is a max-heap, so we reverse the ordering for earliest-first.
impl Ord for ScheduledEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other.next_run.cmp(&self.next_run)
    }
}

// ---------------------------------------------------------------------------
// SchedulerEntry — per-gorilla scheduling state
// ---------------------------------------------------------------------------

/// Per-gorilla scheduling state tracked by the scheduler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerEntry {
    /// The parsed schedule.
    pub schedule: Schedule,
    /// Whether this gorilla is paused.
    pub paused: bool,
    /// When the gorilla last ran.
    pub last_run: Option<DateTime<Utc>>,
    /// When the gorilla is next scheduled to run.
    pub next_run: Option<DateTime<Utc>>,
    /// Total number of runs.
    pub run_count: u64,
    /// Missed run handling policy.
    pub missed_policy: MissedRunPolicy,
    /// Whether this is a one-shot schedule (remove after running).
    pub one_shot: bool,
}

// ---------------------------------------------------------------------------
// GorillaScheduler
// ---------------------------------------------------------------------------

/// Thread-safe cron-based scheduler for gorilla execution.
pub struct GorillaScheduler {
    /// Per-gorilla scheduling entries.
    entries: DashMap<GorillaId, SchedulerEntry>,
    /// Priority queue of next run times (protected by RwLock for async access).
    queue: RwLock<BinaryHeap<ScheduledEntry>>,
    /// Notification channel to wake the scheduler loop when entries change.
    notify: Arc<Notify>,
}

impl GorillaScheduler {
    /// Create a new gorilla scheduler.
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
            queue: RwLock::new(BinaryHeap::new()),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Register a gorilla with the scheduler.
    pub async fn register(
        &self,
        gorilla_id: GorillaId,
        schedule: Schedule,
        missed_policy: MissedRunPolicy,
    ) -> PunchResult<()> {
        let now = Utc::now();
        let one_shot = matches!(schedule, Schedule::OneShot(_));
        let next_run = calculate_next_run(&schedule, now);

        let entry = SchedulerEntry {
            schedule,
            paused: false,
            last_run: None,
            next_run,
            run_count: 0,
            missed_policy,
            one_shot,
        };

        self.entries.insert(gorilla_id, entry);

        if let Some(next) = next_run {
            let mut queue = self.queue.write().await;
            queue.push(ScheduledEntry {
                gorilla_id,
                next_run: next,
            });
        }

        self.notify.notify_one();
        info!(gorilla_id = %gorilla_id, "gorilla registered with scheduler");
        Ok(())
    }

    /// Unregister a gorilla from the scheduler.
    pub async fn unregister(&self, gorilla_id: &GorillaId) {
        self.entries.remove(gorilla_id);
        // The queue will be cleaned up lazily when entries are popped.
        info!(gorilla_id = %gorilla_id, "gorilla unregistered from scheduler");
    }

    /// Pause a gorilla's schedule.
    pub fn pause(&self, gorilla_id: &GorillaId) -> PunchResult<()> {
        let mut entry = self
            .entries
            .get_mut(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not in scheduler", gorilla_id)))?;
        entry.paused = true;
        info!(gorilla_id = %gorilla_id, "gorilla schedule paused");
        Ok(())
    }

    /// Resume a gorilla's schedule.
    pub async fn resume(&self, gorilla_id: &GorillaId) -> PunchResult<()> {
        let mut entry = self
            .entries
            .get_mut(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not in scheduler", gorilla_id)))?;
        entry.paused = false;

        // Recalculate next run.
        let now = Utc::now();
        let next = calculate_next_run(&entry.schedule, now);
        entry.next_run = next;
        let gid = *gorilla_id;
        drop(entry);

        if let Some(next) = next {
            let mut queue = self.queue.write().await;
            queue.push(ScheduledEntry {
                gorilla_id: gid,
                next_run: next,
            });
        }

        self.notify.notify_one();
        info!(gorilla_id = %gorilla_id, "gorilla schedule resumed");
        Ok(())
    }

    /// Record that a gorilla has completed a run and schedule the next one.
    pub async fn record_run(&self, gorilla_id: &GorillaId) -> PunchResult<()> {
        let mut entry = self
            .entries
            .get_mut(gorilla_id)
            .ok_or_else(|| PunchError::Gorilla(format!("gorilla {} not in scheduler", gorilla_id)))?;

        let now = Utc::now();
        entry.last_run = Some(now);
        entry.run_count += 1;

        if entry.one_shot {
            entry.next_run = None;
            debug!(gorilla_id = %gorilla_id, "one-shot gorilla completed, no reschedule");
            return Ok(());
        }

        let next = calculate_next_run(&entry.schedule, now);
        entry.next_run = next;
        drop(entry);

        if let Some(next) = next {
            let mut queue = self.queue.write().await;
            queue.push(ScheduledEntry {
                gorilla_id: *gorilla_id,
                next_run: next,
            });
            self.notify.notify_one();
        }

        Ok(())
    }

    /// Get the next gorilla that is due to run.
    ///
    /// Returns `None` if no gorillas are due. Skips paused gorillas and
    /// gorillas that have been unregistered.
    pub async fn next_due(&self) -> Option<GorillaId> {
        let now = Utc::now();
        let mut queue = self.queue.write().await;

        while let Some(entry) = queue.peek() {
            if entry.next_run > now {
                return None;
            }

            let entry = queue.pop()?;
            let gorilla_id = entry.gorilla_id;

            // Check if gorilla is still registered and not paused.
            if let Some(sched_entry) = self.entries.get(&gorilla_id) {
                if sched_entry.paused {
                    continue;
                }
                return Some(gorilla_id);
            }
            // Gorilla was unregistered; skip this entry.
        }

        None
    }

    /// Get the time until the next scheduled run across all gorillas.
    pub async fn time_until_next(&self) -> Option<Duration> {
        let now = Utc::now();
        let queue = self.queue.read().await;
        queue.peek().and_then(|entry| {
            let diff = entry.next_run - now;
            if diff.num_milliseconds() <= 0 {
                Some(Duration::from_millis(0))
            } else {
                diff.to_std().ok()
            }
        })
    }

    /// Get scheduling info for a gorilla.
    pub fn get_entry(&self, gorilla_id: &GorillaId) -> Option<SchedulerEntry> {
        self.entries.get(gorilla_id).map(|e| e.clone())
    }

    /// List all scheduled gorillas.
    pub fn list_entries(&self) -> Vec<(GorillaId, SchedulerEntry)> {
        self.entries
            .iter()
            .map(|e| (*e.key(), e.value().clone()))
            .collect()
    }

    /// Get the notification handle for waking the scheduler loop.
    pub fn notifier(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }

    /// Handle missed runs for a gorilla according to its policy.
    pub async fn handle_missed_runs(&self, gorilla_id: &GorillaId) -> Vec<DateTime<Utc>> {
        let entry = match self.entries.get(gorilla_id) {
            Some(e) => e.clone(),
            None => return Vec::new(),
        };

        if entry.paused {
            return Vec::new();
        }

        let now = Utc::now();
        let last_run = entry.last_run.unwrap_or(now - chrono::Duration::hours(1));
        let mut missed = Vec::new();

        match entry.missed_policy {
            MissedRunPolicy::Skip => {
                // Nothing to catch up on.
            }
            MissedRunPolicy::CatchUp => {
                // Find all missed run times.
                let mut check = last_run;
                while let Some(next) = calculate_next_run(&entry.schedule, check) {
                    if next >= now {
                        break;
                    }
                    missed.push(next);
                    check = next;
                    // Safety limit.
                    if missed.len() >= 100 {
                        warn!(
                            gorilla_id = %gorilla_id,
                            "too many missed runs, capping at 100"
                        );
                        break;
                    }
                }
            }
            MissedRunPolicy::RunOnce => {
                // Check if there was any missed run.
                if let Some(next) = calculate_next_run(&entry.schedule, last_run)
                    && next < now
                {
                    missed.push(now);
                }
            }
        }

        missed
    }
}

impl Default for GorillaScheduler {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculate the next run time for a schedule after the given time.
fn calculate_next_run(schedule: &Schedule, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match schedule {
        Schedule::Cron(cron) => cron.next_after(after),
        Schedule::Interval(dur) => {
            let dur_chrono = chrono::Duration::from_std(*dur).ok()?;
            Some(after + dur_chrono)
        }
        Schedule::OneShot(time) => {
            if *time > after {
                Some(*time)
            } else {
                None
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cron_field_star() {
        let result = parse_cron_field("*", 0, 59).unwrap();
        assert_eq!(result.len(), 60);
        assert_eq!(result[0], 0);
        assert_eq!(result[59], 59);
    }

    #[test]
    fn parse_cron_field_step() {
        let result = parse_cron_field("*/5", 0, 59).unwrap();
        assert_eq!(result, vec![0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55]);
    }

    #[test]
    fn parse_cron_field_range() {
        let result = parse_cron_field("1-5", 0, 59).unwrap();
        assert_eq!(result, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn parse_cron_field_list() {
        let result = parse_cron_field("1,3,5", 0, 59).unwrap();
        assert_eq!(result, vec![1, 3, 5]);
    }

    #[test]
    fn parse_cron_field_single() {
        let result = parse_cron_field("30", 0, 59).unwrap();
        assert_eq!(result, vec![30]);
    }

    #[test]
    fn parse_cron_field_out_of_range() {
        let result = parse_cron_field("60", 0, 59);
        assert!(result.is_err());
    }

    #[test]
    fn parse_cron_field_zero_step() {
        let result = parse_cron_field("*/0", 0, 59);
        assert!(result.is_err());
    }

    #[test]
    fn parse_cron_expression_every_5_minutes() {
        let cron = CronExpression::parse("*/5 * * * *").unwrap();
        assert_eq!(cron.minutes.len(), 12);
        assert_eq!(cron.hours.len(), 24);
    }

    #[test]
    fn parse_cron_expression_every_6_hours() {
        let cron = CronExpression::parse("0 */6 * * *").unwrap();
        assert_eq!(cron.minutes, vec![0]);
        assert_eq!(cron.hours, vec![0, 6, 12, 18]);
    }

    #[test]
    fn parse_cron_expression_invalid_fields() {
        let result = CronExpression::parse("*/5 * *");
        assert!(result.is_err());
    }

    #[test]
    fn cron_next_after_basic() {
        let cron = CronExpression::parse("0 * * * *").unwrap(); // every hour at :00
        let now = chrono::NaiveDate::from_ymd_opt(2025, 1, 1)
            .unwrap()
            .and_hms_opt(12, 30, 0)
            .unwrap()
            .and_utc();
        let next = cron.next_after(now).unwrap();
        assert_eq!(next.hour(), 13);
        assert_eq!(next.minute(), 0);
    }

    #[test]
    fn cron_next_after_specific_minute() {
        let cron = CronExpression::parse("30 * * * *").unwrap(); // every hour at :30
        let now = chrono::NaiveDate::from_ymd_opt(2025, 1, 1)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc();
        let next = cron.next_after(now).unwrap();
        assert_eq!(next.hour(), 12);
        assert_eq!(next.minute(), 30);
    }

    #[test]
    fn parse_schedule_every_5m() {
        let schedule = parse_schedule("@every 5m").unwrap();
        match schedule {
            Schedule::Interval(d) => assert_eq!(d, Duration::from_secs(300)),
            _ => panic!("expected interval"),
        }
    }

    #[test]
    fn parse_schedule_hourly() {
        let schedule = parse_schedule("@hourly").unwrap();
        match schedule {
            Schedule::Interval(d) => assert_eq!(d, Duration::from_secs(3600)),
            _ => panic!("expected interval"),
        }
    }

    #[test]
    fn parse_schedule_daily() {
        let schedule = parse_schedule("@daily").unwrap();
        match schedule {
            Schedule::Interval(d) => assert_eq!(d, Duration::from_secs(86400)),
            _ => panic!("expected interval"),
        }
    }

    #[test]
    fn parse_schedule_weekly() {
        let schedule = parse_schedule("@weekly").unwrap();
        match schedule {
            Schedule::Interval(d) => assert_eq!(d, Duration::from_secs(604800)),
            _ => panic!("expected interval"),
        }
    }

    #[test]
    fn parse_schedule_cron() {
        let schedule = parse_schedule("0 */6 * * *").unwrap();
        assert!(matches!(schedule, Schedule::Cron(_)));
    }

    #[test]
    fn parse_schedule_every_prefix() {
        let schedule = parse_schedule("every 30s").unwrap();
        match schedule {
            Schedule::Interval(d) => assert_eq!(d, Duration::from_secs(30)),
            _ => panic!("expected interval"),
        }
    }

    #[test]
    fn parse_schedule_raw_duration() {
        let schedule = parse_schedule("10m").unwrap();
        match schedule {
            Schedule::Interval(d) => assert_eq!(d, Duration::from_secs(600)),
            _ => panic!("expected interval"),
        }
    }

    #[test]
    fn parse_schedule_invalid() {
        let result = parse_schedule("not a schedule");
        assert!(result.is_err());
    }

    #[test]
    fn parse_duration_str_seconds() {
        assert_eq!(parse_duration_str("30s"), Some(Duration::from_secs(30)));
    }

    #[test]
    fn parse_duration_str_minutes() {
        assert_eq!(parse_duration_str("5m"), Some(Duration::from_secs(300)));
    }

    #[test]
    fn parse_duration_str_hours() {
        assert_eq!(parse_duration_str("2h"), Some(Duration::from_secs(7200)));
    }

    #[test]
    fn parse_duration_str_days() {
        assert_eq!(parse_duration_str("1d"), Some(Duration::from_secs(86400)));
    }

    #[test]
    fn parse_duration_str_raw() {
        assert_eq!(parse_duration_str("60"), Some(Duration::from_secs(60)));
    }

    #[test]
    fn parse_duration_str_invalid() {
        assert_eq!(parse_duration_str("abc"), None);
    }

    #[tokio::test]
    async fn scheduler_register_and_list() {
        let scheduler = GorillaScheduler::new();
        let id = GorillaId::new();
        scheduler
            .register(id, Schedule::Interval(Duration::from_secs(60)), MissedRunPolicy::Skip)
            .await
            .unwrap();

        let entries = scheduler.list_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, id);
    }

    #[tokio::test]
    async fn scheduler_unregister() {
        let scheduler = GorillaScheduler::new();
        let id = GorillaId::new();
        scheduler
            .register(id, Schedule::Interval(Duration::from_secs(60)), MissedRunPolicy::Skip)
            .await
            .unwrap();

        scheduler.unregister(&id).await;
        assert!(scheduler.get_entry(&id).is_none());
    }

    #[tokio::test]
    async fn scheduler_pause_resume() {
        let scheduler = GorillaScheduler::new();
        let id = GorillaId::new();
        scheduler
            .register(id, Schedule::Interval(Duration::from_secs(60)), MissedRunPolicy::Skip)
            .await
            .unwrap();

        scheduler.pause(&id).unwrap();
        let entry = scheduler.get_entry(&id).unwrap();
        assert!(entry.paused);

        scheduler.resume(&id).await.unwrap();
        let entry = scheduler.get_entry(&id).unwrap();
        assert!(!entry.paused);
    }

    #[tokio::test]
    async fn scheduler_pause_nonexistent() {
        let scheduler = GorillaScheduler::new();
        let id = GorillaId::new();
        assert!(scheduler.pause(&id).is_err());
    }

    #[tokio::test]
    async fn scheduler_resume_nonexistent() {
        let scheduler = GorillaScheduler::new();
        let id = GorillaId::new();
        assert!(scheduler.resume(&id).await.is_err());
    }

    #[tokio::test]
    async fn scheduler_record_run() {
        let scheduler = GorillaScheduler::new();
        let id = GorillaId::new();
        scheduler
            .register(id, Schedule::Interval(Duration::from_secs(60)), MissedRunPolicy::Skip)
            .await
            .unwrap();

        scheduler.record_run(&id).await.unwrap();
        let entry = scheduler.get_entry(&id).unwrap();
        assert_eq!(entry.run_count, 1);
        assert!(entry.last_run.is_some());
    }

    #[tokio::test]
    async fn scheduler_one_shot_removes_after_run() {
        let scheduler = GorillaScheduler::new();
        let id = GorillaId::new();
        let future_time = Utc::now() + chrono::Duration::hours(1);
        scheduler
            .register(id, Schedule::OneShot(future_time), MissedRunPolicy::Skip)
            .await
            .unwrap();

        let entry = scheduler.get_entry(&id).unwrap();
        assert!(entry.one_shot);

        scheduler.record_run(&id).await.unwrap();
        let entry = scheduler.get_entry(&id).unwrap();
        assert!(entry.next_run.is_none());
    }

    #[tokio::test]
    async fn scheduler_missed_runs_skip() {
        let scheduler = GorillaScheduler::new();
        let id = GorillaId::new();
        scheduler
            .register(id, Schedule::Interval(Duration::from_secs(60)), MissedRunPolicy::Skip)
            .await
            .unwrap();

        let missed = scheduler.handle_missed_runs(&id).await;
        assert!(missed.is_empty());
    }

    #[tokio::test]
    async fn scheduler_missed_runs_run_once() {
        let scheduler = GorillaScheduler::new();
        let id = GorillaId::new();
        scheduler
            .register(id, Schedule::Interval(Duration::from_secs(1)), MissedRunPolicy::RunOnce)
            .await
            .unwrap();

        // Set last_run to the past.
        if let Some(mut entry) = scheduler.entries.get_mut(&id) {
            entry.last_run = Some(Utc::now() - chrono::Duration::hours(1));
        }

        let missed = scheduler.handle_missed_runs(&id).await;
        assert_eq!(missed.len(), 1);
    }

    #[tokio::test]
    async fn scheduler_time_until_next() {
        let scheduler = GorillaScheduler::new();
        let id = GorillaId::new();
        scheduler
            .register(id, Schedule::Interval(Duration::from_secs(3600)), MissedRunPolicy::Skip)
            .await
            .unwrap();

        let time = scheduler.time_until_next().await;
        assert!(time.is_some());
        // Should be roughly 1 hour.
        let secs = time.unwrap().as_secs();
        assert!(secs > 3500 && secs <= 3600);
    }

    #[test]
    fn scheduled_entry_ordering() {
        let earlier = ScheduledEntry {
            gorilla_id: GorillaId::new(),
            next_run: Utc::now(),
        };
        let later = ScheduledEntry {
            gorilla_id: GorillaId::new(),
            next_run: Utc::now() + chrono::Duration::hours(1),
        };
        // In a max-heap with reversed Ord, earlier should be "greater" (popped first).
        assert!(earlier > later);
    }

    #[test]
    fn calculate_next_run_interval() {
        let now = Utc::now();
        let schedule = Schedule::Interval(Duration::from_secs(300));
        let next = calculate_next_run(&schedule, now).unwrap();
        let diff = (next - now).num_seconds();
        assert_eq!(diff, 300);
    }

    #[test]
    fn calculate_next_run_one_shot_future() {
        let now = Utc::now();
        let future = now + chrono::Duration::hours(1);
        let schedule = Schedule::OneShot(future);
        let next = calculate_next_run(&schedule, now).unwrap();
        assert_eq!(next, future);
    }

    #[test]
    fn calculate_next_run_one_shot_past() {
        let now = Utc::now();
        let past = now - chrono::Duration::hours(1);
        let schedule = Schedule::OneShot(past);
        let next = calculate_next_run(&schedule, now);
        assert!(next.is_none());
    }

    #[test]
    fn gorilla_scheduler_default() {
        let scheduler = GorillaScheduler::default();
        assert!(scheduler.list_entries().is_empty());
    }
}
