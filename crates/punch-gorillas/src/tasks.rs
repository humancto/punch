//! Gorilla Task System — task queue with priorities, deduplication, and dependencies.
//!
//! Provides a managed task queue for gorillas with priority levels, task
//! deduplication, dependency tracking, timeout management, cancellation,
//! and result persistence.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, Notify};
use tracing::{debug, info, warn};

use punch_types::{GorillaId, PunchError, PunchResult};

// ---------------------------------------------------------------------------
// Task types
// ---------------------------------------------------------------------------

/// Unique identifier for a gorilla task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub uuid::Uuid);

impl TaskId {
    /// Create a new random task ID.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Priority levels for gorilla tasks.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord,
)]
pub enum TaskPriority {
    /// Lowest priority.
    Low = 0,
    /// Normal priority (default).
    #[default]
    Normal = 1,
    /// High priority.
    High = 2,
    /// Highest priority, preempts other tasks.
    Critical = 3,
}

/// Current state of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskState {
    /// Waiting to be executed.
    Pending,
    /// Currently being executed.
    Running,
    /// Successfully completed.
    Completed,
    /// Failed with an error.
    Failed,
    /// Cancelled by the user or system.
    Cancelled,
    /// Timed out during execution.
    TimedOut,
    /// Waiting for dependency tasks to complete.
    Blocked,
}

/// The result of a completed task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// Whether the task succeeded.
    pub success: bool,
    /// Output data from the task.
    pub output: String,
    /// Error message if the task failed.
    pub error: Option<String>,
    /// When the result was produced.
    pub completed_at: DateTime<Utc>,
}

/// A task submitted to a gorilla.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GorillaTask {
    /// Unique task ID.
    pub id: TaskId,
    /// Which gorilla should execute this task.
    pub gorilla_id: GorillaId,
    /// Human-readable task description.
    pub description: String,
    /// Task priority.
    pub priority: TaskPriority,
    /// Current state.
    pub state: TaskState,
    /// When the task was created.
    pub created_at: DateTime<Utc>,
    /// When the task started executing.
    pub started_at: Option<DateTime<Utc>>,
    /// Maximum execution time.
    pub timeout: Option<Duration>,
    /// Task IDs that must complete before this task can run.
    pub depends_on: Vec<TaskId>,
    /// A deduplication key. If set, only one task with this key can be pending.
    pub dedup_key: Option<String>,
    /// The task result, if completed.
    pub result: Option<TaskResult>,
}

// ---------------------------------------------------------------------------
// TaskQueue
// ---------------------------------------------------------------------------

/// Thread-safe task queue for gorilla tasks.
///
/// Tasks are ordered by priority (Critical first) and then by creation time
/// (FIFO within the same priority level).
pub struct TaskQueue {
    /// All tasks indexed by ID.
    tasks: Mutex<HashMap<TaskId, GorillaTask>>,
    /// Priority queues — one deque per priority level.
    queues: Mutex<HashMap<TaskPriority, VecDeque<TaskId>>>,
    /// Deduplication index: dedup_key → TaskId.
    dedup_index: Mutex<HashMap<String, TaskId>>,
    /// Notification for new tasks.
    notify: Arc<Notify>,
}

impl TaskQueue {
    /// Create a new empty task queue.
    pub fn new() -> Self {
        let mut queues = HashMap::new();
        queues.insert(TaskPriority::Critical, VecDeque::new());
        queues.insert(TaskPriority::High, VecDeque::new());
        queues.insert(TaskPriority::Normal, VecDeque::new());
        queues.insert(TaskPriority::Low, VecDeque::new());

        Self {
            tasks: Mutex::new(HashMap::new()),
            queues: Mutex::new(queues),
            dedup_index: Mutex::new(HashMap::new()),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Submit a single task to the queue.
    ///
    /// Returns the task ID, or an error if a duplicate task exists with the
    /// same dedup key.
    pub async fn submit(&self, task: GorillaTask) -> PunchResult<TaskId> {
        let task_id = task.id;

        // Check deduplication.
        if let Some(ref key) = task.dedup_key {
            let dedup = self.dedup_index.lock().await;
            if let Some(existing_id) = dedup.get(key) {
                let tasks = self.tasks.lock().await;
                if let Some(existing) = tasks.get(existing_id)
                    && (existing.state == TaskState::Pending
                        || existing.state == TaskState::Running)
                {
                    return Err(PunchError::Gorilla(format!(
                        "duplicate task with key '{}' already exists (id: {})",
                        key, existing_id
                    )));
                }
            }
        }

        // Determine initial state based on dependencies.
        let initial_state = if task.depends_on.is_empty() {
            TaskState::Pending
        } else {
            let tasks = self.tasks.lock().await;
            let all_deps_met = task.depends_on.iter().all(|dep_id| {
                tasks
                    .get(dep_id)
                    .is_some_and(|t| t.state == TaskState::Completed)
            });
            if all_deps_met {
                TaskState::Pending
            } else {
                TaskState::Blocked
            }
        };

        let mut stored_task = task;
        stored_task.state = initial_state;
        let priority = stored_task.priority;

        // Add to dedup index.
        if let Some(ref key) = stored_task.dedup_key {
            let mut dedup = self.dedup_index.lock().await;
            dedup.insert(key.clone(), task_id);
        }

        // Add to tasks map.
        let mut tasks = self.tasks.lock().await;
        tasks.insert(task_id, stored_task);

        // Add to priority queue if not blocked.
        if initial_state == TaskState::Pending {
            let mut queues = self.queues.lock().await;
            if let Some(queue) = queues.get_mut(&priority) {
                queue.push_back(task_id);
            }
        }

        self.notify.notify_one();
        debug!(task_id = %task_id, ?priority, "task submitted");
        Ok(task_id)
    }

    /// Submit a batch of tasks.
    pub async fn submit_batch(&self, tasks: Vec<GorillaTask>) -> PunchResult<Vec<TaskId>> {
        let mut ids = Vec::with_capacity(tasks.len());
        for task in tasks {
            ids.push(self.submit(task).await?);
        }
        Ok(ids)
    }

    /// Dequeue the next task to execute (highest priority first, FIFO within priority).
    pub async fn dequeue(&self) -> Option<GorillaTask> {
        let priorities = [
            TaskPriority::Critical,
            TaskPriority::High,
            TaskPriority::Normal,
            TaskPriority::Low,
        ];

        let mut queues = self.queues.lock().await;
        let mut tasks = self.tasks.lock().await;

        for priority in &priorities {
            if let Some(queue) = queues.get_mut(priority) {
                while let Some(task_id) = queue.pop_front() {
                    if let Some(task) = tasks.get_mut(&task_id)
                        && task.state == TaskState::Pending
                    {
                        task.state = TaskState::Running;
                        task.started_at = Some(Utc::now());
                        return Some(task.clone());
                    }
                }
            }
        }

        None
    }

    /// Complete a task with a result.
    pub async fn complete(&self, task_id: &TaskId, result: TaskResult) -> PunchResult<()> {
        let mut tasks = self.tasks.lock().await;
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| PunchError::Gorilla(format!("task {} not found", task_id)))?;

        task.state = if result.success {
            TaskState::Completed
        } else {
            TaskState::Failed
        };
        task.result = Some(result);

        // Unblock dependent tasks.
        let completed_id = *task_id;
        let mut to_unblock = Vec::new();
        for (id, t) in tasks.iter() {
            if t.state == TaskState::Blocked && t.depends_on.contains(&completed_id) {
                to_unblock.push(*id);
            }
        }

        for id in &to_unblock {
            if let Some(blocked_task) = tasks.get(id) {
                let all_deps_met = blocked_task.depends_on.iter().all(|dep_id| {
                    tasks
                        .get(dep_id)
                        .is_some_and(|t| t.state == TaskState::Completed)
                });
                if all_deps_met {
                    let priority = blocked_task.priority;
                    if let Some(t) = tasks.get_mut(id) {
                        t.state = TaskState::Pending;
                    }
                    // Add to the appropriate priority queue.
                    let mut queues = self.queues.lock().await;
                    if let Some(queue) = queues.get_mut(&priority) {
                        queue.push_back(*id);
                    }
                }
            }
        }

        info!(task_id = %task_id, "task completed");
        Ok(())
    }

    /// Cancel a task.
    pub async fn cancel(&self, task_id: &TaskId) -> PunchResult<()> {
        let mut tasks = self.tasks.lock().await;
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| PunchError::Gorilla(format!("task {} not found", task_id)))?;

        if task.state == TaskState::Completed || task.state == TaskState::Failed {
            return Err(PunchError::Gorilla(format!(
                "task {} is already in terminal state {:?}",
                task_id, task.state
            )));
        }

        task.state = TaskState::Cancelled;
        info!(task_id = %task_id, "task cancelled");
        Ok(())
    }

    /// Mark a task as timed out.
    pub async fn timeout(&self, task_id: &TaskId) -> PunchResult<()> {
        let mut tasks = self.tasks.lock().await;
        let task = tasks
            .get_mut(task_id)
            .ok_or_else(|| PunchError::Gorilla(format!("task {} not found", task_id)))?;

        task.state = TaskState::TimedOut;
        task.result = Some(TaskResult {
            success: false,
            output: String::new(),
            error: Some("task timed out".to_string()),
            completed_at: Utc::now(),
        });

        warn!(task_id = %task_id, "task timed out");
        Ok(())
    }

    /// Get a task by ID.
    pub async fn get(&self, task_id: &TaskId) -> Option<GorillaTask> {
        let tasks = self.tasks.lock().await;
        tasks.get(task_id).cloned()
    }

    /// Get the result of a task.
    pub async fn get_result(&self, task_id: &TaskId) -> Option<TaskResult> {
        let tasks = self.tasks.lock().await;
        tasks.get(task_id).and_then(|t| t.result.clone())
    }

    /// List all tasks for a gorilla.
    pub async fn list_for_gorilla(&self, gorilla_id: &GorillaId) -> Vec<GorillaTask> {
        let tasks = self.tasks.lock().await;
        tasks
            .values()
            .filter(|t| t.gorilla_id == *gorilla_id)
            .cloned()
            .collect()
    }

    /// Count pending tasks.
    pub async fn pending_count(&self) -> usize {
        let tasks = self.tasks.lock().await;
        tasks
            .values()
            .filter(|t| t.state == TaskState::Pending)
            .count()
    }

    /// Count running tasks.
    pub async fn running_count(&self) -> usize {
        let tasks = self.tasks.lock().await;
        tasks
            .values()
            .filter(|t| t.state == TaskState::Running)
            .count()
    }

    /// Get the notification handle for new tasks.
    pub fn notifier(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }

    /// Check and timeout any tasks that have exceeded their timeout.
    pub async fn check_timeouts(&self) -> Vec<TaskId> {
        let now = Utc::now();
        let mut timed_out = Vec::new();

        let mut tasks = self.tasks.lock().await;
        for (id, task) in tasks.iter_mut() {
            if task.state == TaskState::Running
                && let (Some(started), Some(timeout)) = (task.started_at, task.timeout)
                && let Ok(elapsed) = (now - started).to_std()
                && elapsed > timeout
            {
                task.state = TaskState::TimedOut;
                task.result = Some(TaskResult {
                    success: false,
                    output: String::new(),
                    error: Some("task timed out".to_string()),
                    completed_at: now,
                });
                timed_out.push(*id);
            }
        }

        if !timed_out.is_empty() {
            warn!(count = timed_out.len(), "tasks timed out");
        }

        timed_out
    }
}

impl Default for TaskQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// Create a new task builder for convenience.
pub fn new_task(gorilla_id: GorillaId, description: &str) -> GorillaTask {
    GorillaTask {
        id: TaskId::new(),
        gorilla_id,
        description: description.to_string(),
        priority: TaskPriority::Normal,
        state: TaskState::Pending,
        created_at: Utc::now(),
        started_at: None,
        timeout: None,
        depends_on: Vec::new(),
        dedup_key: None,
        result: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(gorilla_id: GorillaId, desc: &str) -> GorillaTask {
        new_task(gorilla_id, desc)
    }

    fn make_task_with_priority(
        gorilla_id: GorillaId,
        desc: &str,
        priority: TaskPriority,
    ) -> GorillaTask {
        let mut task = new_task(gorilla_id, desc);
        task.priority = priority;
        task
    }

    #[tokio::test]
    async fn submit_and_dequeue() {
        let queue = TaskQueue::new();
        let gid = GorillaId::new();
        let task = make_task(gid, "test task");
        let id = queue.submit(task).await.unwrap();

        let dequeued = queue.dequeue().await.unwrap();
        assert_eq!(dequeued.id, id);
        assert_eq!(dequeued.state, TaskState::Running);
    }

    #[tokio::test]
    async fn priority_ordering() {
        let queue = TaskQueue::new();
        let gid = GorillaId::new();

        let low = make_task_with_priority(gid, "low", TaskPriority::Low);
        let high = make_task_with_priority(gid, "high", TaskPriority::High);
        let critical = make_task_with_priority(gid, "critical", TaskPriority::Critical);

        queue.submit(low).await.unwrap();
        queue.submit(high).await.unwrap();
        queue.submit(critical).await.unwrap();

        let first = queue.dequeue().await.unwrap();
        assert_eq!(first.description, "critical");

        let second = queue.dequeue().await.unwrap();
        assert_eq!(second.description, "high");

        let third = queue.dequeue().await.unwrap();
        assert_eq!(third.description, "low");
    }

    #[tokio::test]
    async fn deduplication() {
        let queue = TaskQueue::new();
        let gid = GorillaId::new();

        let mut task1 = make_task(gid, "first");
        task1.dedup_key = Some("unique-key".to_string());
        queue.submit(task1).await.unwrap();

        let mut task2 = make_task(gid, "duplicate");
        task2.dedup_key = Some("unique-key".to_string());
        let result = queue.submit(task2).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn deduplication_allows_after_completion() {
        let queue = TaskQueue::new();
        let gid = GorillaId::new();

        let mut task1 = make_task(gid, "first");
        task1.dedup_key = Some("key".to_string());
        let id1 = queue.submit(task1).await.unwrap();

        // Complete the first task.
        queue.dequeue().await;
        queue
            .complete(
                &id1,
                TaskResult {
                    success: true,
                    output: "done".to_string(),
                    error: None,
                    completed_at: Utc::now(),
                },
            )
            .await
            .unwrap();

        // Now a duplicate key should be accepted.
        let mut task2 = make_task(gid, "second");
        task2.dedup_key = Some("key".to_string());
        let result = queue.submit(task2).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn task_dependencies() {
        let queue = TaskQueue::new();
        let gid = GorillaId::new();

        let task_a = make_task(gid, "task A");
        let id_a = task_a.id;

        let mut task_b = make_task(gid, "task B");
        task_b.depends_on = vec![id_a];

        queue.submit(task_a).await.unwrap();
        queue.submit(task_b).await.unwrap();

        // Only task A should be dequeued (task B is blocked).
        let first = queue.dequeue().await.unwrap();
        assert_eq!(first.description, "task A");

        let second = queue.dequeue().await;
        assert!(second.is_none()); // task B is still blocked.

        // Complete task A.
        queue
            .complete(
                &id_a,
                TaskResult {
                    success: true,
                    output: "done".to_string(),
                    error: None,
                    completed_at: Utc::now(),
                },
            )
            .await
            .unwrap();

        // Now task B should be available.
        let unblocked = queue.dequeue().await.unwrap();
        assert_eq!(unblocked.description, "task B");
    }

    #[tokio::test]
    async fn cancel_task() {
        let queue = TaskQueue::new();
        let gid = GorillaId::new();
        let task = make_task(gid, "cancellable");
        let id = queue.submit(task).await.unwrap();

        queue.cancel(&id).await.unwrap();

        let task = queue.get(&id).await.unwrap();
        assert_eq!(task.state, TaskState::Cancelled);
    }

    #[tokio::test]
    async fn cancel_completed_task_fails() {
        let queue = TaskQueue::new();
        let gid = GorillaId::new();
        let task = make_task(gid, "done");
        let id = queue.submit(task).await.unwrap();

        queue.dequeue().await;
        queue
            .complete(
                &id,
                TaskResult {
                    success: true,
                    output: "done".to_string(),
                    error: None,
                    completed_at: Utc::now(),
                },
            )
            .await
            .unwrap();

        assert!(queue.cancel(&id).await.is_err());
    }

    #[tokio::test]
    async fn timeout_task() {
        let queue = TaskQueue::new();
        let gid = GorillaId::new();
        let task = make_task(gid, "slow");
        let id = queue.submit(task).await.unwrap();

        queue.timeout(&id).await.unwrap();

        let task = queue.get(&id).await.unwrap();
        assert_eq!(task.state, TaskState::TimedOut);
    }

    #[tokio::test]
    async fn check_timeouts() {
        let queue = TaskQueue::new();
        let gid = GorillaId::new();

        let mut task = make_task(gid, "timeout test");
        task.timeout = Some(Duration::from_millis(1));
        let id = queue.submit(task).await.unwrap();

        // Dequeue to set it as Running.
        queue.dequeue().await;

        // Wait a tiny bit for the timeout to expire.
        tokio::time::sleep(Duration::from_millis(10)).await;

        let timed_out = queue.check_timeouts().await;
        assert_eq!(timed_out.len(), 1);
        assert_eq!(timed_out[0], id);
    }

    #[tokio::test]
    async fn pending_and_running_counts() {
        let queue = TaskQueue::new();
        let gid = GorillaId::new();

        queue.submit(make_task(gid, "a")).await.unwrap();
        queue.submit(make_task(gid, "b")).await.unwrap();
        assert_eq!(queue.pending_count().await, 2);
        assert_eq!(queue.running_count().await, 0);

        queue.dequeue().await;
        assert_eq!(queue.pending_count().await, 1);
        assert_eq!(queue.running_count().await, 1);
    }

    #[tokio::test]
    async fn list_for_gorilla() {
        let queue = TaskQueue::new();
        let gid1 = GorillaId::new();
        let gid2 = GorillaId::new();

        queue.submit(make_task(gid1, "a")).await.unwrap();
        queue.submit(make_task(gid1, "b")).await.unwrap();
        queue.submit(make_task(gid2, "c")).await.unwrap();

        let list1 = queue.list_for_gorilla(&gid1).await;
        assert_eq!(list1.len(), 2);

        let list2 = queue.list_for_gorilla(&gid2).await;
        assert_eq!(list2.len(), 1);
    }

    #[tokio::test]
    async fn get_result() {
        let queue = TaskQueue::new();
        let gid = GorillaId::new();
        let task = make_task(gid, "result test");
        let id = queue.submit(task).await.unwrap();

        assert!(queue.get_result(&id).await.is_none());

        queue.dequeue().await;
        queue
            .complete(
                &id,
                TaskResult {
                    success: true,
                    output: "output data".to_string(),
                    error: None,
                    completed_at: Utc::now(),
                },
            )
            .await
            .unwrap();

        let result = queue.get_result(&id).await.unwrap();
        assert!(result.success);
        assert_eq!(result.output, "output data");
    }

    #[tokio::test]
    async fn batch_submit() {
        let queue = TaskQueue::new();
        let gid = GorillaId::new();

        let tasks = vec![
            make_task(gid, "batch-1"),
            make_task(gid, "batch-2"),
            make_task(gid, "batch-3"),
        ];

        let ids = queue.submit_batch(tasks).await.unwrap();
        assert_eq!(ids.len(), 3);
        assert_eq!(queue.pending_count().await, 3);
    }

    #[tokio::test]
    async fn empty_dequeue_returns_none() {
        let queue = TaskQueue::new();
        assert!(queue.dequeue().await.is_none());
    }

    #[test]
    fn task_id_display() {
        let id = TaskId::new();
        let s = format!("{}", id);
        assert!(!s.is_empty());
    }

    #[test]
    fn task_id_default() {
        let id = TaskId::default();
        assert!(!id.0.is_nil());
    }

    #[test]
    fn task_priority_ordering() {
        assert!(TaskPriority::Critical > TaskPriority::High);
        assert!(TaskPriority::High > TaskPriority::Normal);
        assert!(TaskPriority::Normal > TaskPriority::Low);
    }

    #[test]
    fn task_priority_default() {
        assert_eq!(TaskPriority::default(), TaskPriority::Normal);
    }

    #[test]
    fn task_queue_default() {
        let _queue = TaskQueue::default();
    }

    #[test]
    fn new_task_helper() {
        let gid = GorillaId::new();
        let task = new_task(gid, "test");
        assert_eq!(task.gorilla_id, gid);
        assert_eq!(task.description, "test");
        assert_eq!(task.priority, TaskPriority::Normal);
        assert_eq!(task.state, TaskState::Pending);
        assert!(task.depends_on.is_empty());
        assert!(task.dedup_key.is_none());
        assert!(task.result.is_none());
    }

    #[test]
    fn task_result_serialization() {
        let result = TaskResult {
            success: true,
            output: "done".to_string(),
            error: None,
            completed_at: Utc::now(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let deser: TaskResult = serde_json::from_str(&json).unwrap();
        assert!(deser.success);
    }

    #[test]
    fn gorilla_task_serialization() {
        let gid = GorillaId::new();
        let task = new_task(gid, "serialize me");
        let json = serde_json::to_string(&task).unwrap();
        let deser: GorillaTask = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.description, "serialize me");
    }
}
