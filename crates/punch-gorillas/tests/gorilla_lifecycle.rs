//! Integration tests for gorilla lifecycle, scheduler, executor, and task queue.
//!
//! Tests cover manifest parsing, scheduling, execution state transitions,
//! circuit breaker behavior, and task queue priority ordering.

use std::time::Duration;

use punch_gorillas::executor::{ExecutorConfig, ExecutionRecord};
use punch_gorillas::scheduler::CronExpression;
use punch_gorillas::tasks::{
    GorillaTask, TaskId, TaskPriority, TaskQueue, TaskResult, TaskState, new_task,
};
use punch_gorillas::GorillaLifecycle;
use punch_types::{GorillaId, GorillaManifest, GorillaStatus};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_manifest(name: &str) -> GorillaManifest {
    GorillaManifest {
        name: name.to_string(),
        description: format!("{name} gorilla"),
        schedule: "*/5 * * * *".to_string(),
        moves_required: vec!["read_file".to_string()],
        settings_schema: None,
        dashboard_metrics: vec!["uptime".to_string()],
        system_prompt: None,
        model: None,
        capabilities: Vec::new(),
        weight_class: None,
    }
}

// ---------------------------------------------------------------------------
// Manifest parsing tests
// ---------------------------------------------------------------------------

/// Parse a gorilla manifest from TOML and verify all fields.
#[test]
fn test_parse_gorilla_manifest_from_toml() {
    let toml_str = r#"
name = "DataSweeper"
description = "Cleans up old files"
schedule = "0 */6 * * *"
moves_required = ["read_file", "write_file", "shell_exec"]
dashboard_metrics = ["files_cleaned", "bytes_freed"]
system_prompt = "You clean up stale data."

[settings]
type = "object"
"#;

    // Parse as GorillaManifest (using serde with toml).
    let manifest: GorillaManifest = toml::from_str(toml_str).expect("should parse TOML manifest");

    assert_eq!(manifest.name, "DataSweeper");
    assert_eq!(manifest.description, "Cleans up old files");
    assert_eq!(manifest.schedule, "0 */6 * * *");
    assert_eq!(manifest.moves_required.len(), 3);
    assert!(manifest.moves_required.contains(&"read_file".to_string()));
    assert!(manifest.moves_required.contains(&"shell_exec".to_string()));
    assert_eq!(manifest.dashboard_metrics.len(), 2);
    assert_eq!(
        manifest.effective_system_prompt(),
        "You clean up stale data."
    );
    assert!(manifest.model.is_none());
    assert!(manifest.weight_class.is_none());
}

// ---------------------------------------------------------------------------
// Lifecycle tests
// ---------------------------------------------------------------------------

/// Register a gorilla, verify it appears in list.
#[tokio::test]
async fn test_register_gorilla_appears_in_list() {
    let lifecycle = GorillaLifecycle::new();
    let id = lifecycle.register(test_manifest("Watcher")).await;

    let list = lifecycle.list().await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].0, id);
    assert_eq!(list[0].1, "Watcher");
    assert_eq!(list[0].2, GorillaStatus::Caged);
}

/// Unleash a gorilla and verify status transitions to Unleashed.
#[tokio::test]
async fn test_unleash_gorilla_status_change() {
    let lifecycle = GorillaLifecycle::new();
    let id = lifecycle.register(test_manifest("Runner")).await;

    lifecycle.unleash(id).await.expect("unleash should succeed");
    let status = lifecycle.get_status(id).await.unwrap();
    assert_eq!(status, GorillaStatus::Unleashed);
}

/// Cage a running gorilla and verify status returns to Caged.
#[tokio::test]
async fn test_cage_running_gorilla() {
    let lifecycle = GorillaLifecycle::new();
    let id = lifecycle.register(test_manifest("Pauser")).await;

    lifecycle.unleash(id).await.unwrap();
    assert_eq!(lifecycle.get_status(id).await.unwrap(), GorillaStatus::Unleashed);

    lifecycle.cage(id).await.unwrap();
    assert_eq!(lifecycle.get_status(id).await.unwrap(), GorillaStatus::Caged);
}

/// Attempt to unleash a non-existent gorilla and verify error.
#[tokio::test]
async fn test_unleash_nonexistent_gorilla_errors() {
    let lifecycle = GorillaLifecycle::new();
    let fake_id = GorillaId::new();
    let result = lifecycle.unleash(fake_id).await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Scheduler (cron) tests
// ---------------------------------------------------------------------------

/// Parse a standard 5-field cron expression.
#[test]
fn test_cron_parse_standard_expression() {
    let cron = CronExpression::parse("0 */6 * * *").expect("should parse");
    assert_eq!(cron.minutes, vec![0]);
    assert_eq!(cron.hours, vec![0, 6, 12, 18]);
    assert_eq!(cron.days_of_month.len(), 31);
    assert_eq!(cron.months.len(), 12);
    assert_eq!(cron.days_of_week.len(), 7);
}

/// Verify next_after produces a valid future time.
#[test]
fn test_cron_next_after_is_in_future() {
    let cron = CronExpression::parse("*/5 * * * *").expect("should parse");
    let now = chrono::Utc::now();
    let next = cron.next_after(now);
    assert!(next.is_some(), "next_after should produce a result");
    assert!(next.unwrap() > now, "next run should be in the future");
}

// ---------------------------------------------------------------------------
// Executor config tests
// ---------------------------------------------------------------------------

/// Default executor config has sensible values.
#[test]
fn test_executor_config_defaults() {
    let config = ExecutorConfig::default();
    assert_eq!(config.circuit_breaker_threshold, 5);
    assert_eq!(config.max_retries, 3);
    assert!(config.execution_timeout.as_secs() > 0);
    assert!(config.max_history_entries > 0);
}

/// Execution record serialization roundtrip.
#[test]
fn test_execution_record_serde() {
    let record = ExecutionRecord {
        started_at: chrono::Utc::now(),
        completed_at: chrono::Utc::now(),
        success: true,
        duration: Duration::from_secs(5),
        error: None,
        summary: Some("All good".to_string()),
        retries: 0,
    };
    let json = serde_json::to_string(&record).expect("serialize");
    let deser: ExecutionRecord = serde_json::from_str(&json).expect("deserialize");
    assert!(deser.success);
    assert_eq!(deser.retries, 0);
    assert_eq!(deser.summary, Some("All good".to_string()));
}

// ---------------------------------------------------------------------------
// Task queue tests
// ---------------------------------------------------------------------------

/// Submit 5 tasks with mixed priorities and verify dequeue order.
#[tokio::test]
async fn test_task_queue_priority_ordering() {
    let queue = TaskQueue::new();
    let gid = GorillaId::new();

    // Submit tasks in non-priority order.
    let mut t_low = new_task(gid, "low-task");
    t_low.priority = TaskPriority::Low;

    let mut t_normal = new_task(gid, "normal-task");
    t_normal.priority = TaskPriority::Normal;

    let mut t_high = new_task(gid, "high-task");
    t_high.priority = TaskPriority::High;

    let mut t_critical = new_task(gid, "critical-task");
    t_critical.priority = TaskPriority::Critical;

    let mut t_normal2 = new_task(gid, "normal-task-2");
    t_normal2.priority = TaskPriority::Normal;

    // Submit in scrambled order.
    queue.submit(t_normal).await.unwrap();
    queue.submit(t_low).await.unwrap();
    queue.submit(t_critical).await.unwrap();
    queue.submit(t_normal2).await.unwrap();
    queue.submit(t_high).await.unwrap();

    // Dequeue should yield: Critical, High, Normal, Normal2, Low.
    let d1 = queue.dequeue().await.unwrap();
    assert_eq!(d1.description, "critical-task");

    let d2 = queue.dequeue().await.unwrap();
    assert_eq!(d2.description, "high-task");

    let d3 = queue.dequeue().await.unwrap();
    assert_eq!(d3.description, "normal-task");

    let d4 = queue.dequeue().await.unwrap();
    assert_eq!(d4.description, "normal-task-2");

    let d5 = queue.dequeue().await.unwrap();
    assert_eq!(d5.description, "low-task");

    // Queue should be empty now.
    assert!(queue.dequeue().await.is_none());
}

/// Submit a task, complete it, verify result retrieval.
#[tokio::test]
async fn test_task_complete_and_get_result() {
    let queue = TaskQueue::new();
    let gid = GorillaId::new();
    let task = new_task(gid, "result-check");
    let id = queue.submit(task).await.unwrap();

    queue.dequeue().await; // Sets to Running

    queue
        .complete(
            &id,
            TaskResult {
                success: true,
                output: "completed output".to_string(),
                error: None,
                completed_at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();

    let result = queue.get_result(&id).await.unwrap();
    assert!(result.success);
    assert_eq!(result.output, "completed output");
}
