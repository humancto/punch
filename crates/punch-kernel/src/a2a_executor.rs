//! A2A Task Executor — picks up pending A2A tasks and runs them through fighters.
//!
//! The [`A2ATaskExecutor`] polls a shared [`DashMap`] of tasks for any in
//! [`Pending`](punch_types::a2a::A2ATaskStatus::Pending) status, spawns a
//! temporary fighter for each, executes the task, and writes the result back.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use dashmap::DashMap;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{error, info, instrument};

use punch_types::a2a::{A2ATask, A2ATaskInput, A2ATaskOutput, A2ATaskStatus};
use punch_types::{FighterManifest, WeightClass};

use crate::ring::Ring;

/// Default polling interval for the executor (500ms).
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// The A2A task executor: polls for pending tasks and executes them via fighters.
pub struct A2ATaskExecutor {
    /// Reference to the Ring for spawning fighters and sending messages.
    ring: Arc<Ring>,
    /// Shared task map (same instance as the HTTP handlers use).
    tasks: Arc<DashMap<String, A2ATask>>,
    /// Polling interval.
    poll_interval: Duration,
    /// Shutdown signal sender.
    shutdown_tx: watch::Sender<bool>,
    /// Shutdown signal receiver (cloned for the polling task).
    shutdown_rx: watch::Receiver<bool>,
    /// Handle to the background polling task.
    handle: Option<JoinHandle<()>>,
}

impl A2ATaskExecutor {
    /// Create a new executor that will poll the given task map and use the Ring
    /// to spawn fighters for execution.
    pub fn new(ring: Arc<Ring>, tasks: Arc<DashMap<String, A2ATask>>) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            ring,
            tasks,
            poll_interval: DEFAULT_POLL_INTERVAL,
            shutdown_tx,
            shutdown_rx,
            handle: None,
        }
    }

    /// Create a new executor with a custom polling interval.
    pub fn with_poll_interval(
        ring: Arc<Ring>,
        tasks: Arc<DashMap<String, A2ATask>>,
        poll_interval: Duration,
    ) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        Self {
            ring,
            tasks,
            poll_interval,
            shutdown_tx,
            shutdown_rx,
            handle: None,
        }
    }

    /// Start the background polling loop.
    ///
    /// Spawns a tokio task that polls the DashMap every `poll_interval` for
    /// [`Pending`](A2ATaskStatus::Pending) tasks. Each pending task is picked
    /// up and executed in its own spawned task.
    pub fn start(&mut self) {
        let ring = Arc::clone(&self.ring);
        let tasks = Arc::clone(&self.tasks);
        let interval = self.poll_interval;
        let mut shutdown_rx = self.shutdown_rx.clone();

        let handle = tokio::spawn(async move {
            info!(
                poll_interval_ms = interval.as_millis(),
                "A2A task executor started"
            );

            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            info!("A2A task executor received shutdown signal");
                            break;
                        }
                    }
                }

                if *shutdown_rx.borrow() {
                    break;
                }

                // Collect pending task IDs (avoid holding DashMap guards across await).
                let pending_ids: Vec<String> = tasks
                    .iter()
                    .filter(|entry| entry.value().status == A2ATaskStatus::Pending)
                    .map(|entry| entry.key().clone())
                    .collect();

                for task_id in pending_ids {
                    // Transition to Running (atomic check-and-set).
                    let task_input = {
                        let mut entry = match tasks.get_mut(&task_id) {
                            Some(e) => e,
                            None => continue,
                        };
                        // Double-check it's still Pending (another poll may have grabbed it).
                        if entry.status != A2ATaskStatus::Pending {
                            continue;
                        }
                        entry.status = A2ATaskStatus::Running;
                        entry.updated_at = Utc::now();
                        entry.input.clone()
                    };

                    // Spawn execution in a separate task so we don't block polling.
                    let ring = Arc::clone(&ring);
                    let tasks = Arc::clone(&tasks);
                    let id = task_id.clone();

                    tokio::spawn(async move {
                        execute_task(ring, tasks, id, task_input).await;
                    });
                }
            }

            info!("A2A task executor stopped");
        });

        self.handle = Some(handle);
    }

    /// Stop the polling loop.
    pub fn stop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        info!("A2A task executor stop requested");
    }

    /// Returns `true` if the executor is currently running.
    pub fn is_running(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| !h.is_finished())
    }
}

impl Drop for A2ATaskExecutor {
    fn drop(&mut self) {
        // Best-effort shutdown on drop.
        let _ = self.shutdown_tx.send(true);
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// Execute a single A2A task: spawn a fighter, send the prompt, collect the
/// result, and update the DashMap.
#[instrument(skip(ring, tasks, task_input), fields(task_id = %task_id))]
async fn execute_task(
    ring: Arc<Ring>,
    tasks: Arc<DashMap<String, A2ATask>>,
    task_id: String,
    task_input: serde_json::Value,
) {
    // Extract the prompt from the input.
    let prompt = extract_prompt(&task_input);

    // Build a temporary fighter manifest for this task.
    let manifest = FighterManifest {
        name: format!("a2a-task-{}", &task_id[..8.min(task_id.len())]),
        description: format!("Temporary fighter for A2A task {task_id}"),
        model: ring.config().default_model.clone(),
        system_prompt: build_task_system_prompt(&task_input),
        capabilities: Vec::new(),
        weight_class: WeightClass::Middleweight,
        tenant_id: None,
    };

    // Spawn the fighter.
    let fighter_id = ring.spawn_fighter(manifest).await;

    // Send the prompt and collect the result.
    let result = ring.send_message(&fighter_id, prompt).await;

    // Update the task based on the result.
    match result {
        Ok(loop_result) => {
            if let Some(mut entry) = tasks.get_mut(&task_id) {
                // Don't overwrite a Cancelled task.
                if entry.status == A2ATaskStatus::Cancelled {
                    info!(task_id = %task_id, "task was cancelled during execution, skipping update");
                } else {
                    let output = A2ATaskOutput {
                        content: loop_result.response.clone(),
                        data: Some(serde_json::json!({
                            "tokens_used": loop_result.usage.total(),
                            "iterations": loop_result.iterations,
                            "tool_calls": loop_result.tool_calls_made,
                        })),
                        mode: "text".to_string(),
                    };
                    entry.status = A2ATaskStatus::Completed;
                    entry.output =
                        Some(serde_json::to_value(output).unwrap_or(serde_json::json!({})));
                    entry.updated_at = Utc::now();
                    info!(task_id = %task_id, "A2A task completed successfully");
                }
            }
        }
        Err(e) => {
            error!(task_id = %task_id, error = %e, "A2A task execution failed");
            if let Some(mut entry) = tasks.get_mut(&task_id)
                && entry.status != A2ATaskStatus::Cancelled
            {
                entry.status = A2ATaskStatus::Failed(e.to_string());
                entry.updated_at = Utc::now();
            }
        }
    }

    // Kill the temporary fighter.
    ring.kill_fighter(&fighter_id);
}

/// Extract the prompt text from a task input JSON value.
///
/// Tries to parse as [`A2ATaskInput`] first, then falls back to looking for a
/// "prompt" field, and finally uses the JSON as a string.
fn extract_prompt(input: &serde_json::Value) -> String {
    // Try structured A2ATaskInput.
    if let Ok(structured) = serde_json::from_value::<A2ATaskInput>(input.clone()) {
        return structured.prompt;
    }

    // Try a "prompt" field directly.
    if let Some(prompt) = input.get("prompt").and_then(|v| v.as_str()) {
        return prompt.to_string();
    }

    // Try a "message" field.
    if let Some(msg) = input.get("message").and_then(|v| v.as_str()) {
        return msg.to_string();
    }

    // Fall back to the JSON as a string.
    if let Some(s) = input.as_str() {
        return s.to_string();
    }

    input.to_string()
}

/// Build a system prompt for the task fighter, incorporating any context from
/// the task input.
fn build_task_system_prompt(input: &serde_json::Value) -> String {
    let mut prompt = "You are an AI agent executing a task received via the A2A protocol. \
                      Complete the task thoroughly and return a clear, actionable response."
        .to_string();

    // If the input has a context object, include it.
    if let Some(context) = input.get("context")
        && let Some(obj) = context.as_object()
        && !obj.is_empty()
    {
        prompt.push_str("\n\n## Task Context\n");
        for (key, value) in obj {
            prompt.push_str(&format!("- **{key}**: {value}\n"));
        }
    }

    prompt
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::a2a::A2ATaskStatus;

    fn make_task(id: &str, status: A2ATaskStatus) -> A2ATask {
        let now = Utc::now();
        A2ATask {
            id: id.to_string(),
            status,
            input: serde_json::json!({"prompt": "hello world"}),
            output: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn test_extract_prompt_structured() {
        let input = serde_json::json!({
            "prompt": "Summarize this code",
            "context": {},
            "mode": "text"
        });
        assert_eq!(extract_prompt(&input), "Summarize this code");
    }

    #[test]
    fn test_extract_prompt_simple_prompt_field() {
        let input = serde_json::json!({"prompt": "Do the thing"});
        assert_eq!(extract_prompt(&input), "Do the thing");
    }

    #[test]
    fn test_extract_prompt_message_field() {
        let input = serde_json::json!({"message": "Hello agent"});
        assert_eq!(extract_prompt(&input), "Hello agent");
    }

    #[test]
    fn test_extract_prompt_string_value() {
        let input = serde_json::json!("Just a string prompt");
        assert_eq!(extract_prompt(&input), "Just a string prompt");
    }

    #[test]
    fn test_extract_prompt_fallback_json() {
        let input = serde_json::json!({"arbitrary": "data", "count": 42});
        let result = extract_prompt(&input);
        assert!(result.contains("arbitrary"));
    }

    #[test]
    fn test_build_task_system_prompt_no_context() {
        let input = serde_json::json!({"prompt": "hello"});
        let prompt = build_task_system_prompt(&input);
        assert!(prompt.contains("A2A protocol"));
        assert!(!prompt.contains("Task Context"));
    }

    #[test]
    fn test_build_task_system_prompt_with_context() {
        let input = serde_json::json!({
            "prompt": "hello",
            "context": {
                "language": "rust",
                "project": "punch"
            }
        });
        let prompt = build_task_system_prompt(&input);
        assert!(prompt.contains("Task Context"));
        assert!(prompt.contains("language"));
        assert!(prompt.contains("rust"));
    }

    #[test]
    fn test_build_task_system_prompt_empty_context() {
        let input = serde_json::json!({
            "prompt": "hello",
            "context": {}
        });
        let prompt = build_task_system_prompt(&input);
        assert!(!prompt.contains("Task Context"));
    }

    #[test]
    fn test_executor_creation() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        // We can't easily create a real Ring in unit tests, so we test the
        // components that don't require one.
        assert_eq!(tasks.len(), 0);
    }

    #[test]
    fn test_task_pending_to_running_transition() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        let task = make_task("task-001", A2ATaskStatus::Pending);
        tasks.insert("task-001".to_string(), task);

        // Simulate the executor picking up the task.
        {
            let mut entry = tasks.get_mut("task-001").unwrap();
            assert_eq!(entry.status, A2ATaskStatus::Pending);
            entry.status = A2ATaskStatus::Running;
            entry.updated_at = Utc::now();
        }

        let entry = tasks.get("task-001").unwrap();
        assert_eq!(entry.status, A2ATaskStatus::Running);
    }

    #[test]
    fn test_task_running_to_completed_transition() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        let task = make_task("task-002", A2ATaskStatus::Running);
        tasks.insert("task-002".to_string(), task);

        // Simulate successful completion.
        {
            let mut entry = tasks.get_mut("task-002").unwrap();
            let output = A2ATaskOutput {
                content: "Task result here".to_string(),
                data: None,
                mode: "text".to_string(),
            };
            entry.status = A2ATaskStatus::Completed;
            entry.output = Some(serde_json::to_value(output).unwrap());
            entry.updated_at = Utc::now();
        }

        let entry = tasks.get("task-002").unwrap();
        assert_eq!(entry.status, A2ATaskStatus::Completed);
        assert!(entry.output.is_some());
    }

    #[test]
    fn test_task_running_to_failed_transition() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        let task = make_task("task-003", A2ATaskStatus::Running);
        tasks.insert("task-003".to_string(), task);

        // Simulate failure.
        {
            let mut entry = tasks.get_mut("task-003").unwrap();
            entry.status = A2ATaskStatus::Failed("LLM provider error".to_string());
            entry.updated_at = Utc::now();
        }

        let entry = tasks.get("task-003").unwrap();
        assert!(
            matches!(entry.status, A2ATaskStatus::Failed(ref msg) if msg.contains("LLM provider"))
        );
    }

    #[test]
    fn test_multiple_concurrent_tasks() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());

        // Insert multiple pending tasks.
        for i in 0..5 {
            let task = make_task(&format!("concurrent-{i}"), A2ATaskStatus::Pending);
            tasks.insert(format!("concurrent-{i}"), task);
        }

        // Collect pending IDs (simulating executor poll).
        let pending_ids: Vec<String> = tasks
            .iter()
            .filter(|e| e.status == A2ATaskStatus::Pending)
            .map(|e| e.key().clone())
            .collect();

        assert_eq!(pending_ids.len(), 5);

        // Transition all to Running.
        for id in &pending_ids {
            let mut entry = tasks.get_mut(id).unwrap();
            entry.status = A2ATaskStatus::Running;
        }

        // Verify all are Running.
        let running_count = tasks
            .iter()
            .filter(|e| e.status == A2ATaskStatus::Running)
            .count();
        assert_eq!(running_count, 5);

        // Complete some, fail others.
        for (i, id) in pending_ids.iter().enumerate() {
            let mut entry = tasks.get_mut(id).unwrap();
            if i % 2 == 0 {
                entry.status = A2ATaskStatus::Completed;
                entry.output = Some(serde_json::json!({"result": "ok"}));
            } else {
                entry.status = A2ATaskStatus::Failed("test error".to_string());
            }
        }

        let completed = tasks
            .iter()
            .filter(|e| e.status == A2ATaskStatus::Completed)
            .count();
        let failed = tasks
            .iter()
            .filter(|e| matches!(e.status, A2ATaskStatus::Failed(_)))
            .count();
        assert_eq!(completed, 3);
        assert_eq!(failed, 2);
    }

    #[test]
    fn test_cancelled_task_not_overwritten() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        let task = make_task("task-cancel", A2ATaskStatus::Running);
        tasks.insert("task-cancel".to_string(), task);

        // Cancel the task while it's "running".
        {
            let mut entry = tasks.get_mut("task-cancel").unwrap();
            entry.status = A2ATaskStatus::Cancelled;
            entry.updated_at = Utc::now();
        }

        // Simulate executor trying to write a result after cancellation.
        {
            let mut entry = tasks.get_mut("task-cancel").unwrap();
            if entry.status != A2ATaskStatus::Cancelled {
                entry.status = A2ATaskStatus::Completed;
                entry.output = Some(serde_json::json!({"result": "should not appear"}));
            }
        }

        let entry = tasks.get("task-cancel").unwrap();
        assert_eq!(entry.status, A2ATaskStatus::Cancelled);
        assert!(entry.output.is_none());
    }

    #[test]
    fn test_completed_task_has_output() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        let task = make_task("task-output", A2ATaskStatus::Running);
        tasks.insert("task-output".to_string(), task);

        let output = A2ATaskOutput {
            content: "The answer is 42".to_string(),
            data: Some(serde_json::json!({"tokens_used": 100})),
            mode: "text".to_string(),
        };

        {
            let mut entry = tasks.get_mut("task-output").unwrap();
            entry.status = A2ATaskStatus::Completed;
            entry.output = Some(serde_json::to_value(&output).unwrap());
            entry.updated_at = Utc::now();
        }

        let entry = tasks.get("task-output").unwrap();
        assert_eq!(entry.status, A2ATaskStatus::Completed);
        let stored_output = entry.output.as_ref().unwrap();
        assert_eq!(stored_output["content"], "The answer is 42");
        assert_eq!(stored_output["mode"], "text");
    }

    #[test]
    fn test_failed_task_has_error_message() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        let task = make_task("task-err", A2ATaskStatus::Running);
        tasks.insert("task-err".to_string(), task);

        {
            let mut entry = tasks.get_mut("task-err").unwrap();
            entry.status = A2ATaskStatus::Failed("connection timeout to provider".to_string());
            entry.updated_at = Utc::now();
        }

        let entry = tasks.get("task-err").unwrap();
        match &entry.status {
            A2ATaskStatus::Failed(msg) => {
                assert!(msg.contains("connection timeout"));
            }
            _ => panic!("expected Failed status"),
        }
    }

    #[tokio::test]
    async fn test_stop_cancellation() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        // Simulate executor components without a real Ring.
        let mut shutdown_rx_clone = shutdown_rx.clone();

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(50)) => {}
                    _ = shutdown_rx_clone.changed() => {
                        if *shutdown_rx_clone.borrow() {
                            break;
                        }
                    }
                }
                if *shutdown_rx_clone.borrow() {
                    break;
                }
            }
        });

        // Let it run briefly.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!handle.is_finished());

        // Send shutdown.
        let _ = shutdown_tx.send(true);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(handle.is_finished());

        // Tasks map should still be accessible.
        assert_eq!(tasks.len(), 0);
    }

    #[test]
    fn test_pending_task_skipped_if_already_claimed() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        let task = make_task("task-race", A2ATaskStatus::Pending);
        tasks.insert("task-race".to_string(), task);

        // First "poll" claims it.
        {
            let mut entry = tasks.get_mut("task-race").unwrap();
            if entry.status == A2ATaskStatus::Pending {
                entry.status = A2ATaskStatus::Running;
            }
        }

        // Second "poll" should see Running, not Pending.
        {
            let entry = tasks.get("task-race").unwrap();
            assert_eq!(entry.status, A2ATaskStatus::Running);
        }

        // Collect pending — should be empty now.
        let pending: Vec<String> = tasks
            .iter()
            .filter(|e| e.status == A2ATaskStatus::Pending)
            .map(|e| e.key().clone())
            .collect();
        assert!(pending.is_empty());
    }

    #[test]
    fn test_extract_prompt_with_context_and_prompt() {
        let input = serde_json::json!({
            "prompt": "Analyze this code",
            "context": {
                "language": "rust"
            },
            "mode": "text"
        });
        assert_eq!(extract_prompt(&input), "Analyze this code");
    }

    #[test]
    fn test_extract_prompt_numeric_value() {
        let input = serde_json::json!(42);
        let result = extract_prompt(&input);
        assert_eq!(result, "42");
    }

    #[test]
    fn test_extract_prompt_null_value() {
        let input = serde_json::json!(null);
        let result = extract_prompt(&input);
        assert_eq!(result, "null");
    }

    #[test]
    fn test_extract_prompt_array_value() {
        let input = serde_json::json!(["a", "b"]);
        let result = extract_prompt(&input);
        assert!(result.contains('a'));
    }

    #[test]
    fn test_extract_prompt_empty_object() {
        let input = serde_json::json!({});
        let result = extract_prompt(&input);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_extract_prompt_prefers_structured_over_prompt_field() {
        // A2ATaskInput has prompt, context, mode fields.
        let input = serde_json::json!({
            "prompt": "structured prompt",
            "context": {},
            "mode": "text"
        });
        assert_eq!(extract_prompt(&input), "structured prompt");
    }

    #[test]
    fn test_extract_prompt_message_over_json_fallback() {
        let input = serde_json::json!({
            "message": "msg field",
            "other": "data"
        });
        assert_eq!(extract_prompt(&input), "msg field");
    }

    #[test]
    fn test_build_task_system_prompt_with_multiple_context_keys() {
        let input = serde_json::json!({
            "prompt": "do stuff",
            "context": {
                "a": "1",
                "b": "2",
                "c": "3"
            }
        });
        let prompt = build_task_system_prompt(&input);
        assert!(prompt.contains("Task Context"));
        assert!(prompt.contains("**a**"));
        assert!(prompt.contains("**b**"));
        assert!(prompt.contains("**c**"));
    }

    #[test]
    fn test_build_task_system_prompt_null_context() {
        let input = serde_json::json!({
            "prompt": "hello",
            "context": null
        });
        let prompt = build_task_system_prompt(&input);
        assert!(!prompt.contains("Task Context"));
    }

    #[test]
    fn test_build_task_system_prompt_context_is_string() {
        let input = serde_json::json!({
            "prompt": "hello",
            "context": "not an object"
        });
        let prompt = build_task_system_prompt(&input);
        assert!(!prompt.contains("Task Context"));
    }

    #[test]
    fn test_task_lifecycle_pending_running_completed() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        let task = make_task("lifecycle", A2ATaskStatus::Pending);
        tasks.insert("lifecycle".to_string(), task);

        // Step 1: Pending -> Running.
        {
            let mut entry = tasks.get_mut("lifecycle").unwrap();
            assert_eq!(entry.status, A2ATaskStatus::Pending);
            entry.status = A2ATaskStatus::Running;
        }

        // Step 2: Running -> Completed.
        {
            let mut entry = tasks.get_mut("lifecycle").unwrap();
            assert_eq!(entry.status, A2ATaskStatus::Running);
            entry.status = A2ATaskStatus::Completed;
            entry.output = Some(serde_json::json!({"result": "done"}));
        }

        let entry = tasks.get("lifecycle").unwrap();
        assert_eq!(entry.status, A2ATaskStatus::Completed);
        assert!(entry.output.is_some());
    }

    #[test]
    fn test_task_lifecycle_pending_running_failed() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        let task = make_task("fail-life", A2ATaskStatus::Pending);
        tasks.insert("fail-life".to_string(), task);

        {
            let mut entry = tasks.get_mut("fail-life").unwrap();
            entry.status = A2ATaskStatus::Running;
        }
        {
            let mut entry = tasks.get_mut("fail-life").unwrap();
            entry.status = A2ATaskStatus::Failed("some error".to_string());
        }

        let entry = tasks.get("fail-life").unwrap();
        assert!(matches!(entry.status, A2ATaskStatus::Failed(_)));
    }

    #[test]
    fn test_failed_task_preserves_error_detail() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        let task = make_task("err-detail", A2ATaskStatus::Running);
        tasks.insert("err-detail".to_string(), task);

        let error_msg = "rate limit exceeded: retry after 60s".to_string();
        {
            let mut entry = tasks.get_mut("err-detail").unwrap();
            entry.status = A2ATaskStatus::Failed(error_msg.clone());
        }

        let entry = tasks.get("err-detail").unwrap();
        match &entry.status {
            A2ATaskStatus::Failed(msg) => assert_eq!(msg, &error_msg),
            _ => panic!("expected Failed"),
        }
    }

    #[test]
    fn test_concurrent_task_isolation() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());

        // Create independent tasks.
        tasks.insert("t1".to_string(), make_task("t1", A2ATaskStatus::Pending));
        tasks.insert("t2".to_string(), make_task("t2", A2ATaskStatus::Running));
        tasks.insert("t3".to_string(), make_task("t3", A2ATaskStatus::Completed));

        // Modifying one doesn't affect others.
        {
            let mut entry = tasks.get_mut("t1").unwrap();
            entry.status = A2ATaskStatus::Running;
        }

        assert_eq!(tasks.get("t1").unwrap().status, A2ATaskStatus::Running);
        assert_eq!(tasks.get("t2").unwrap().status, A2ATaskStatus::Running);
        assert_eq!(tasks.get("t3").unwrap().status, A2ATaskStatus::Completed);
    }

    #[test]
    fn test_task_output_with_structured_data() {
        let output = A2ATaskOutput {
            content: "Result text".to_string(),
            data: Some(serde_json::json!({
                "tokens_used": 500,
                "iterations": 3,
                "tool_calls": 2,
            })),
            mode: "text".to_string(),
        };
        let json = serde_json::to_value(&output).unwrap();
        assert_eq!(json["content"], "Result text");
        assert_eq!(json["data"]["tokens_used"], 500);
        assert_eq!(json["data"]["iterations"], 3);
    }

    #[test]
    fn test_task_removal() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        tasks.insert(
            "rm-task".to_string(),
            make_task("rm-task", A2ATaskStatus::Completed),
        );

        assert!(tasks.contains_key("rm-task"));
        tasks.remove("rm-task");
        assert!(!tasks.contains_key("rm-task"));
    }

    #[test]
    fn test_task_updated_at_changes() {
        let tasks: Arc<DashMap<String, A2ATask>> = Arc::new(DashMap::new());
        let task = make_task("time-task", A2ATaskStatus::Pending);
        let original_time = task.updated_at;
        tasks.insert("time-task".to_string(), task);

        // Small sleep to ensure time difference.
        std::thread::sleep(std::time::Duration::from_millis(10));

        {
            let mut entry = tasks.get_mut("time-task").unwrap();
            entry.status = A2ATaskStatus::Running;
            entry.updated_at = Utc::now();
        }

        let entry = tasks.get("time-task").unwrap();
        assert!(entry.updated_at >= original_time);
    }
}
