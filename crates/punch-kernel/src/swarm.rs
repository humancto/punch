//! # Swarm Intelligence
//!
//! Higher-level coordination for emergent behavior across multiple fighters.
//! The swarm coordinator manages complex tasks by decomposing them into subtasks,
//! assigning them based on capabilities, and aggregating results.

use std::sync::Arc;

use chrono::Utc;
use dashmap::DashMap;
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use punch_types::{
    FighterId, PunchError, PunchResult, SubtaskStatus, SwarmSubtask, SwarmTask,
};

/// How long (in seconds) a running subtask can go without update before
/// being considered stale.
const STALE_THRESHOLD_SECS: i64 = 300;

/// A record of fighter load for balancing purposes.
#[derive(Debug, Clone)]
pub struct FighterLoad {
    /// The fighter's ID.
    pub fighter_id: FighterId,
    /// Number of currently assigned subtasks.
    pub active_tasks: usize,
    /// Whether the fighter is considered healthy.
    pub healthy: bool,
    /// Fighter capabilities for capability-aware assignment.
    pub capabilities: Vec<String>,
    /// Timestamp of last health check (epoch seconds).
    pub last_heartbeat: i64,
}

/// The swarm coordinator manages concurrent swarm tasks across available fighters.
pub struct SwarmCoordinator {
    /// Active swarm tasks keyed by their UUID.
    tasks: DashMap<Uuid, Arc<Mutex<SwarmTask>>>,
    /// Fighter load tracking for balancing.
    fighter_loads: DashMap<FighterId, FighterLoad>,
}

impl SwarmCoordinator {
    /// Create a new swarm coordinator.
    pub fn new() -> Self {
        Self {
            tasks: DashMap::new(),
            fighter_loads: DashMap::new(),
        }
    }

    /// Register a fighter as available for swarm work.
    pub fn register_fighter(&self, fighter_id: FighterId) {
        self.fighter_loads.insert(
            fighter_id,
            FighterLoad {
                fighter_id,
                active_tasks: 0,
                healthy: true,
                capabilities: vec![],
                last_heartbeat: Utc::now().timestamp(),
            },
        );
    }

    /// Register a fighter with specific capabilities.
    pub fn register_fighter_with_capabilities(
        &self,
        fighter_id: FighterId,
        capabilities: Vec<String>,
    ) {
        self.fighter_loads.insert(
            fighter_id,
            FighterLoad {
                fighter_id,
                active_tasks: 0,
                healthy: true,
                capabilities,
                last_heartbeat: Utc::now().timestamp(),
            },
        );
    }

    /// Remove a fighter from the available pool.
    pub fn unregister_fighter(&self, fighter_id: &FighterId) {
        self.fighter_loads.remove(fighter_id);
    }

    /// Record a heartbeat from a fighter to track liveness.
    pub fn record_heartbeat(&self, fighter_id: &FighterId) {
        if let Some(mut load) = self.fighter_loads.get_mut(fighter_id) {
            load.last_heartbeat = Utc::now().timestamp();
        }
    }

    /// Decompose a task description into subtasks.
    ///
    /// Uses intelligent splitting: tries paragraph breaks first, then
    /// sentence boundaries, then falls back to line-based splitting.
    pub fn decompose_task(&self, description: &str) -> Vec<SwarmSubtask> {
        let trimmed = description.trim();
        if trimmed.is_empty() {
            return vec![SwarmSubtask {
                id: Uuid::new_v4(),
                description: description.to_string(),
                assigned_to: None,
                status: SubtaskStatus::Pending,
                result: None,
                depends_on: vec![],
            }];
        }

        // Try splitting by paragraph breaks (double newlines) first.
        let paragraphs: Vec<&str> = trimmed
            .split("\n\n")
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .collect();

        if paragraphs.len() > 1 {
            return paragraphs
                .into_iter()
                .map(|p| SwarmSubtask {
                    id: Uuid::new_v4(),
                    description: p.to_string(),
                    assigned_to: None,
                    status: SubtaskStatus::Pending,
                    result: None,
                    depends_on: vec![],
                })
                .collect();
        }

        // Try splitting by sentences (period followed by space or end).
        let sentences: Vec<&str> = trimmed
            .split(". ")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if sentences.len() > 1 {
            return sentences
                .into_iter()
                .map(|s| SwarmSubtask {
                    id: Uuid::new_v4(),
                    description: s.to_string(),
                    assigned_to: None,
                    status: SubtaskStatus::Pending,
                    result: None,
                    depends_on: vec![],
                })
                .collect();
        }

        // Fall back to line-based splitting.
        let lines: Vec<&str> = trimmed
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();

        if lines.len() > 1 {
            return lines
                .into_iter()
                .map(|line| SwarmSubtask {
                    id: Uuid::new_v4(),
                    description: line.to_string(),
                    assigned_to: None,
                    status: SubtaskStatus::Pending,
                    result: None,
                    depends_on: vec![],
                })
                .collect();
        }

        // Single subtask for atomic tasks.
        vec![SwarmSubtask {
            id: Uuid::new_v4(),
            description: trimmed.to_string(),
            assigned_to: None,
            status: SubtaskStatus::Pending,
            result: None,
            depends_on: vec![],
        }]
    }

    /// Create a new swarm task from a description.
    ///
    /// The task is automatically decomposed into subtasks.
    pub fn create_task(&self, description: String) -> Uuid {
        let subtasks = self.decompose_task(&description);
        let task = SwarmTask {
            id: Uuid::new_v4(),
            description,
            subtasks,
            progress: 0.0,
            created_at: Utc::now(),
            aggregated_result: None,
        };
        let id = task.id;
        self.tasks.insert(id, Arc::new(Mutex::new(task)));
        info!(%id, "swarm task created");
        id
    }

    /// Create a swarm task with explicit subtasks (for pipeline or dependent tasks).
    pub fn create_task_with_subtasks(
        &self,
        description: String,
        subtasks: Vec<SwarmSubtask>,
    ) -> Uuid {
        let task = SwarmTask {
            id: Uuid::new_v4(),
            description,
            subtasks,
            progress: 0.0,
            created_at: Utc::now(),
            aggregated_result: None,
        };
        let id = task.id;
        self.tasks.insert(id, Arc::new(Mutex::new(task)));
        info!(%id, "swarm task created with explicit subtasks");
        id
    }

    /// Assign pending subtasks to available fighters using load balancing
    /// and capability matching.
    ///
    /// Returns a list of (subtask_id, fighter_id) assignments.
    pub async fn assign_subtasks(
        &self,
        task_id: &Uuid,
    ) -> PunchResult<Vec<(Uuid, FighterId)>> {
        let task_ref = self
            .tasks
            .get(task_id)
            .ok_or_else(|| PunchError::Troop(format!("swarm task {} not found", task_id)))?;

        let mut task = task_ref.value().lock().await;
        let mut assignments = Vec::new();

        // First pass: determine which subtasks are eligible for assignment.
        let eligible_indices: Vec<usize> = {
            let subtasks = &task.subtasks;
            subtasks
                .iter()
                .enumerate()
                .filter(|(_, s)| s.status == SubtaskStatus::Pending)
                .filter(|(_, s)| {
                    s.depends_on.iter().all(|dep_id| {
                        subtasks
                            .iter()
                            .any(|d| d.id == *dep_id && d.status == SubtaskStatus::Completed)
                    })
                })
                .map(|(i, _)| i)
                .collect()
        };

        // Second pass: assign eligible subtasks considering capabilities.
        for idx in eligible_indices {
            let subtask_desc = &task.subtasks[idx].description;
            let fighter = self.find_best_fighter_for_task(subtask_desc);

            if let Some(fighter_id) = fighter {
                let subtask = &mut task.subtasks[idx];
                subtask.assigned_to = Some(fighter_id);
                subtask.status = SubtaskStatus::Running;

                if let Some(mut load) = self.fighter_loads.get_mut(&fighter_id) {
                    load.active_tasks += 1;
                }

                assignments.push((subtask.id, fighter_id));
                info!(
                    task_id = %task_id,
                    subtask_id = %subtask.id,
                    fighter_id = %fighter_id,
                    "subtask assigned"
                );
            }
        }

        Ok(assignments)
    }

    /// Find the best fighter for a given task, considering both load and
    /// capability matching.
    fn find_best_fighter_for_task(&self, task_description: &str) -> Option<FighterId> {
        let task_lower = task_description.to_lowercase();

        // First try to find a capable fighter with the lowest load.
        let capable_fighter = self
            .fighter_loads
            .iter()
            .filter(|entry| entry.value().healthy)
            .filter(|entry| {
                // Either has matching capability or has no capabilities (generalist).
                entry.value().capabilities.is_empty()
                    || entry
                        .value()
                        .capabilities
                        .iter()
                        .any(|cap| task_lower.contains(&cap.to_lowercase()))
            })
            .min_by_key(|entry| entry.value().active_tasks)
            .map(|entry| entry.value().fighter_id);

        if capable_fighter.is_some() {
            return capable_fighter;
        }

        // Fall back to any healthy fighter with least load.
        self.find_least_loaded_fighter()
    }

    /// Find the fighter with the lowest active task count.
    fn find_least_loaded_fighter(&self) -> Option<FighterId> {
        self.fighter_loads
            .iter()
            .filter(|entry| entry.value().healthy)
            .min_by_key(|entry| entry.value().active_tasks)
            .map(|entry| entry.value().fighter_id)
    }

    /// Report the completion of a subtask.
    pub async fn complete_subtask(
        &self,
        task_id: &Uuid,
        subtask_id: &Uuid,
        result: String,
    ) -> PunchResult<()> {
        let task_ref = self
            .tasks
            .get(task_id)
            .ok_or_else(|| PunchError::Troop(format!("swarm task {} not found", task_id)))?;

        let mut task = task_ref.value().lock().await;

        let subtask = task
            .subtasks
            .iter_mut()
            .find(|s| s.id == *subtask_id)
            .ok_or_else(|| {
                PunchError::Troop(format!("subtask {} not found in task {}", subtask_id, task_id))
            })?;

        // Decrement load for the assigned fighter.
        if let Some(fighter_id) = subtask.assigned_to
            && let Some(mut load) = self.fighter_loads.get_mut(&fighter_id)
        {
            load.active_tasks = load.active_tasks.saturating_sub(1);
        }

        subtask.status = SubtaskStatus::Completed;
        subtask.result = Some(result);

        // Update overall progress.
        self.update_progress(&mut task);

        // If all subtasks are done, aggregate results intelligently.
        if task.progress >= 1.0 {
            task.aggregated_result = Some(self.aggregate_results(&task.subtasks));
            info!(%task_id, "swarm task fully completed");
        }

        Ok(())
    }

    /// Update the progress field of a task based on subtask completion.
    fn update_progress(&self, task: &mut SwarmTask) {
        let total = task.subtasks.len() as f64;
        let completed = task
            .subtasks
            .iter()
            .filter(|s| s.status == SubtaskStatus::Completed)
            .count() as f64;
        task.progress = if total > 0.0 { completed / total } else { 0.0 };
    }

    /// Aggregate results from completed subtasks intelligently.
    ///
    /// Merges results preserving order and removing redundancy.
    fn aggregate_results(&self, subtasks: &[SwarmSubtask]) -> String {
        let results: Vec<&str> = subtasks
            .iter()
            .filter_map(|s| s.result.as_deref())
            .collect();

        if results.is_empty() {
            return String::new();
        }

        // If results are short, join with double newlines.
        // If long, add headers for each section.
        let total_len: usize = results.iter().map(|r| r.len()).sum();
        if total_len < 500 || results.len() <= 2 {
            results.join("\n\n")
        } else {
            results
                .iter()
                .enumerate()
                .map(|(i, r)| format!("--- Section {} ---\n{}", i + 1, r))
                .collect::<Vec<_>>()
                .join("\n\n")
        }
    }

    /// Report the failure of a subtask.
    pub async fn fail_subtask(
        &self,
        task_id: &Uuid,
        subtask_id: &Uuid,
        error: String,
    ) -> PunchResult<()> {
        let task_ref = self
            .tasks
            .get(task_id)
            .ok_or_else(|| PunchError::Troop(format!("swarm task {} not found", task_id)))?;

        let mut task = task_ref.value().lock().await;

        let subtask = task
            .subtasks
            .iter_mut()
            .find(|s| s.id == *subtask_id)
            .ok_or_else(|| {
                PunchError::Troop(format!("subtask {} not found in task {}", subtask_id, task_id))
            })?;

        // Decrement load for the assigned fighter.
        if let Some(fighter_id) = subtask.assigned_to
            && let Some(mut load) = self.fighter_loads.get_mut(&fighter_id)
        {
            load.active_tasks = load.active_tasks.saturating_sub(1);
        }

        subtask.status = SubtaskStatus::Failed(error);
        warn!(%task_id, %subtask_id, "subtask failed");

        Ok(())
    }

    /// Reassign a failed subtask to a different fighter.
    pub async fn reassign_failed_subtask(
        &self,
        task_id: &Uuid,
        subtask_id: &Uuid,
    ) -> PunchResult<Option<FighterId>> {
        let task_ref = self
            .tasks
            .get(task_id)
            .ok_or_else(|| PunchError::Troop(format!("swarm task {} not found", task_id)))?;

        let mut task = task_ref.value().lock().await;

        let subtask = task
            .subtasks
            .iter_mut()
            .find(|s| s.id == *subtask_id)
            .ok_or_else(|| {
                PunchError::Troop(format!("subtask {} not found in task {}", subtask_id, task_id))
            })?;

        // Only reassign if the subtask has failed.
        if !matches!(subtask.status, SubtaskStatus::Failed(_)) {
            return Err(PunchError::Troop(
                "can only reassign failed subtasks".to_string(),
            ));
        }

        let failed_fighter = subtask.assigned_to;

        // Find a different healthy fighter.
        let new_fighter = self
            .fighter_loads
            .iter()
            .filter(|entry| entry.value().healthy)
            .filter(|entry| Some(entry.value().fighter_id) != failed_fighter)
            .min_by_key(|entry| entry.value().active_tasks)
            .map(|entry| entry.value().fighter_id);

        if let Some(fighter_id) = new_fighter {
            subtask.assigned_to = Some(fighter_id);
            subtask.status = SubtaskStatus::Running;

            if let Some(mut load) = self.fighter_loads.get_mut(&fighter_id) {
                load.active_tasks += 1;
            }

            info!(
                %task_id,
                %subtask_id,
                new_fighter = %fighter_id,
                "subtask reassigned after failure"
            );
        }

        Ok(new_fighter)
    }

    /// Detect stale or failed fighters and reassign their work.
    ///
    /// Returns a list of fighter IDs that were detected as stale/failed.
    pub fn detect_stale_fighters(&self) -> Vec<FighterId> {
        let now = Utc::now().timestamp();
        let mut stale = Vec::new();

        for entry in self.fighter_loads.iter() {
            let load = entry.value();
            if load.healthy
                && load.active_tasks > 0
                && (now - load.last_heartbeat) > STALE_THRESHOLD_SECS
            {
                stale.push(load.fighter_id);
            }
        }

        for fighter_id in &stale {
            self.mark_unhealthy(fighter_id);
            warn!(
                %fighter_id,
                "fighter detected as stale, marked unhealthy"
            );
        }

        stale
    }

    /// Mark a fighter as unhealthy (will not receive new assignments).
    pub fn mark_unhealthy(&self, fighter_id: &FighterId) {
        if let Some(mut load) = self.fighter_loads.get_mut(fighter_id) {
            load.healthy = false;
            warn!(%fighter_id, "fighter marked unhealthy");
        }
    }

    /// Mark a fighter as healthy again.
    pub fn mark_healthy(&self, fighter_id: &FighterId) {
        if let Some(mut load) = self.fighter_loads.get_mut(fighter_id) {
            load.healthy = true;
            load.last_heartbeat = Utc::now().timestamp();
            info!(%fighter_id, "fighter marked healthy");
        }
    }

    /// Get the current state of a swarm task.
    pub async fn get_task(&self, task_id: &Uuid) -> Option<SwarmTask> {
        let task_ref = self.tasks.get(task_id)?;
        let task = task_ref.value().lock().await;
        Some(task.clone())
    }

    /// Get the progress of a swarm task (0.0 to 1.0).
    pub async fn get_progress(&self, task_id: &Uuid) -> Option<f64> {
        let task_ref = self.tasks.get(task_id)?;
        let task = task_ref.value().lock().await;
        Some(task.progress)
    }

    /// Get a detailed progress report for a swarm task.
    pub async fn get_progress_report(&self, task_id: &Uuid) -> Option<ProgressReport> {
        let task_ref = self.tasks.get(task_id)?;
        let task = task_ref.value().lock().await;

        let total = task.subtasks.len();
        let completed = task
            .subtasks
            .iter()
            .filter(|s| s.status == SubtaskStatus::Completed)
            .count();
        let running = task
            .subtasks
            .iter()
            .filter(|s| s.status == SubtaskStatus::Running)
            .count();
        let failed = task
            .subtasks
            .iter()
            .filter(|s| matches!(s.status, SubtaskStatus::Failed(_)))
            .count();
        let pending = task
            .subtasks
            .iter()
            .filter(|s| s.status == SubtaskStatus::Pending)
            .count();

        Some(ProgressReport {
            task_id: *task_id,
            total_subtasks: total,
            completed,
            running,
            failed,
            pending,
            percentage: if total > 0 {
                (completed as f64 / total as f64) * 100.0
            } else {
                0.0
            },
        })
    }

    /// List all active swarm task IDs.
    pub fn list_task_ids(&self) -> Vec<Uuid> {
        self.tasks.iter().map(|entry| *entry.key()).collect()
    }

    /// Get the number of available healthy fighters.
    pub fn available_fighter_count(&self) -> usize {
        self.fighter_loads
            .iter()
            .filter(|entry| entry.value().healthy)
            .count()
    }

    /// Get load information for all fighters.
    pub fn get_fighter_loads(&self) -> Vec<FighterLoad> {
        self.fighter_loads
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Remove a completed or failed swarm task.
    pub fn remove_task(&self, task_id: &Uuid) -> bool {
        self.tasks.remove(task_id).is_some()
    }
}

/// Detailed progress report for a swarm task.
#[derive(Debug, Clone)]
pub struct ProgressReport {
    /// The task ID.
    pub task_id: Uuid,
    /// Total number of subtasks.
    pub total_subtasks: usize,
    /// Number of completed subtasks.
    pub completed: usize,
    /// Number of currently running subtasks.
    pub running: usize,
    /// Number of failed subtasks.
    pub failed: usize,
    /// Number of pending subtasks.
    pub pending: usize,
    /// Completion percentage (0.0 to 100.0).
    pub percentage: f64,
}

impl Default for SwarmCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decompose_single_task() {
        let coord = SwarmCoordinator::new();
        let subtasks = coord.decompose_task("single task");
        assert_eq!(subtasks.len(), 1);
        assert_eq!(subtasks[0].description, "single task");
    }

    #[test]
    fn test_decompose_multi_line_task() {
        let coord = SwarmCoordinator::new();
        let subtasks = coord.decompose_task("step 1\nstep 2\nstep 3");
        assert_eq!(subtasks.len(), 3);
        assert_eq!(subtasks[0].description, "step 1");
        assert_eq!(subtasks[1].description, "step 2");
        assert_eq!(subtasks[2].description, "step 3");
    }

    #[test]
    fn test_decompose_ignores_blank_lines() {
        let coord = SwarmCoordinator::new();
        let subtasks = coord.decompose_task("step 1\n\n\nstep 2\n");
        // Double newline splits into paragraphs.
        assert_eq!(subtasks.len(), 2);
    }

    #[test]
    fn test_decompose_by_paragraphs() {
        let coord = SwarmCoordinator::new();
        let input = "First paragraph about setup.\n\nSecond paragraph about execution.\n\nThird paragraph about cleanup.";
        let subtasks = coord.decompose_task(input);
        assert_eq!(subtasks.len(), 3);
        assert!(subtasks[0].description.contains("setup"));
        assert!(subtasks[1].description.contains("execution"));
        assert!(subtasks[2].description.contains("cleanup"));
    }

    #[test]
    fn test_decompose_by_sentences() {
        let coord = SwarmCoordinator::new();
        let input = "Analyze the code. Fix bugs. Write tests";
        let subtasks = coord.decompose_task(input);
        assert_eq!(subtasks.len(), 3);
    }

    #[test]
    fn test_create_task() {
        let coord = SwarmCoordinator::new();
        let id = coord.create_task("test task".to_string());
        assert!(coord.tasks.contains_key(&id));
    }

    #[test]
    fn test_create_task_with_subtasks() {
        let coord = SwarmCoordinator::new();
        let subtasks = vec![
            SwarmSubtask {
                id: Uuid::new_v4(),
                description: "sub1".to_string(),
                assigned_to: None,
                status: SubtaskStatus::Pending,
                result: None,
                depends_on: vec![],
            },
            SwarmSubtask {
                id: Uuid::new_v4(),
                description: "sub2".to_string(),
                assigned_to: None,
                status: SubtaskStatus::Pending,
                result: None,
                depends_on: vec![],
            },
        ];
        let id = coord.create_task_with_subtasks("parent".to_string(), subtasks);
        assert!(coord.tasks.contains_key(&id));
    }

    #[test]
    fn test_register_and_unregister_fighter() {
        let coord = SwarmCoordinator::new();
        let f = FighterId::new();
        coord.register_fighter(f);
        assert_eq!(coord.available_fighter_count(), 1);
        coord.unregister_fighter(&f);
        assert_eq!(coord.available_fighter_count(), 0);
    }

    #[test]
    fn test_register_fighter_with_capabilities() {
        let coord = SwarmCoordinator::new();
        let f = FighterId::new();
        coord.register_fighter_with_capabilities(
            f,
            vec!["code".to_string(), "review".to_string()],
        );

        let load = coord.fighter_loads.get(&f).expect("should exist");
        assert_eq!(load.capabilities.len(), 2);
        assert!(load.capabilities.contains(&"code".to_string()));
    }

    #[test]
    fn test_find_least_loaded_fighter() {
        let coord = SwarmCoordinator::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        coord.register_fighter(f1);
        coord.register_fighter(f2);

        // Give f1 some load.
        if let Some(mut load) = coord.fighter_loads.get_mut(&f1) {
            load.active_tasks = 5;
        }

        let least = coord.find_least_loaded_fighter();
        assert_eq!(least, Some(f2));
    }

    #[test]
    fn test_find_least_loaded_skips_unhealthy() {
        let coord = SwarmCoordinator::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        coord.register_fighter(f1);
        coord.register_fighter(f2);

        coord.mark_unhealthy(&f2);
        let least = coord.find_least_loaded_fighter();
        assert_eq!(least, Some(f1));
    }

    #[test]
    fn test_mark_healthy_unhealthy() {
        let coord = SwarmCoordinator::new();
        let f = FighterId::new();
        coord.register_fighter(f);
        assert_eq!(coord.available_fighter_count(), 1);

        coord.mark_unhealthy(&f);
        assert_eq!(coord.available_fighter_count(), 0);

        coord.mark_healthy(&f);
        assert_eq!(coord.available_fighter_count(), 1);
    }

    #[tokio::test]
    async fn test_assign_subtasks() {
        let coord = SwarmCoordinator::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        coord.register_fighter(f1);
        coord.register_fighter(f2);

        let task_id = coord.create_task("step 1\nstep 2".to_string());
        let assignments = coord.assign_subtasks(&task_id).await.expect("should assign");
        assert_eq!(assignments.len(), 2);
    }

    #[tokio::test]
    async fn test_assign_subtasks_respects_dependencies() {
        let coord = SwarmCoordinator::new();
        let f = FighterId::new();
        coord.register_fighter(f);

        let dep_id = Uuid::new_v4();
        let subtasks = vec![
            SwarmSubtask {
                id: dep_id,
                description: "first".to_string(),
                assigned_to: None,
                status: SubtaskStatus::Pending,
                result: None,
                depends_on: vec![],
            },
            SwarmSubtask {
                id: Uuid::new_v4(),
                description: "second (depends on first)".to_string(),
                assigned_to: None,
                status: SubtaskStatus::Pending,
                result: None,
                depends_on: vec![dep_id],
            },
        ];
        let task_id = coord.create_task_with_subtasks("pipeline".to_string(), subtasks);

        let assignments = coord.assign_subtasks(&task_id).await.expect("should assign");
        // Only the first subtask (no dependencies) should be assigned.
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].0, dep_id);
    }

    #[tokio::test]
    async fn test_complete_subtask() {
        let coord = SwarmCoordinator::new();
        let f = FighterId::new();
        coord.register_fighter(f);

        let task_id = coord.create_task("single task".to_string());
        let assignments = coord.assign_subtasks(&task_id).await.expect("should assign");
        assert_eq!(assignments.len(), 1);

        let (subtask_id, _) = assignments[0];
        coord
            .complete_subtask(&task_id, &subtask_id, "done".to_string())
            .await
            .expect("should complete");

        let task = coord.get_task(&task_id).await.expect("should exist");
        assert!((task.progress - 1.0).abs() < f64::EPSILON);
        assert!(task.aggregated_result.is_some());
    }

    #[tokio::test]
    async fn test_fail_subtask() {
        let coord = SwarmCoordinator::new();
        let f = FighterId::new();
        coord.register_fighter(f);

        let task_id = coord.create_task("fail task".to_string());
        let assignments = coord.assign_subtasks(&task_id).await.expect("should assign");
        let (subtask_id, _) = assignments[0];

        coord
            .fail_subtask(&task_id, &subtask_id, "error occurred".to_string())
            .await
            .expect("should fail");

        let task = coord.get_task(&task_id).await.expect("should exist");
        assert!(matches!(task.subtasks[0].status, SubtaskStatus::Failed(_)));
    }

    #[tokio::test]
    async fn test_get_progress() {
        let coord = SwarmCoordinator::new();
        let f = FighterId::new();
        coord.register_fighter(f);

        let task_id = coord.create_task("a\nb".to_string());
        let assignments = coord.assign_subtasks(&task_id).await.expect("should assign");

        // Complete first subtask.
        coord
            .complete_subtask(&task_id, &assignments[0].0, "result 1".to_string())
            .await
            .expect("should complete");

        let progress = coord.get_progress(&task_id).await.expect("should exist");
        assert!((progress - 0.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_progress_report() {
        let coord = SwarmCoordinator::new();
        let f = FighterId::new();
        coord.register_fighter(f);

        let task_id = coord.create_task("a\nb\nc".to_string());
        let assignments = coord.assign_subtasks(&task_id).await.expect("should assign");

        // Complete one subtask.
        coord
            .complete_subtask(&task_id, &assignments[0].0, "done".to_string())
            .await
            .expect("should complete");

        let report = coord
            .get_progress_report(&task_id)
            .await
            .expect("should exist");
        assert_eq!(report.total_subtasks, 3);
        assert_eq!(report.completed, 1);
        assert_eq!(report.running, 2);
        assert_eq!(report.pending, 0);
        assert_eq!(report.failed, 0);
        assert!((report.percentage - 33.33).abs() < 1.0);
    }

    #[tokio::test]
    async fn test_load_balancing_distributes_evenly() {
        let coord = SwarmCoordinator::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        let f3 = FighterId::new();
        coord.register_fighter(f1);
        coord.register_fighter(f2);
        coord.register_fighter(f3);

        let task_id = coord.create_task("a\nb\nc\nd\ne\nf".to_string());
        let assignments = coord.assign_subtasks(&task_id).await.expect("should assign");

        assert_eq!(assignments.len(), 6);

        // Count assignments per fighter.
        let mut counts: std::collections::HashMap<FighterId, usize> =
            std::collections::HashMap::new();
        for (_, fighter) in &assignments {
            *counts.entry(*fighter).or_insert(0) += 1;
        }

        // Each fighter should get 2 tasks.
        for count in counts.values() {
            assert_eq!(*count, 2);
        }
    }

    #[tokio::test]
    async fn test_reassign_failed_subtask() {
        let coord = SwarmCoordinator::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        coord.register_fighter(f1);
        coord.register_fighter(f2);

        let task_id = coord.create_task("single task".to_string());
        let assignments = coord.assign_subtasks(&task_id).await.expect("should assign");
        let (subtask_id, original_fighter) = assignments[0];

        // Fail the subtask.
        coord
            .fail_subtask(&task_id, &subtask_id, "crashed".to_string())
            .await
            .expect("should fail");

        // Reassign.
        let new_fighter = coord
            .reassign_failed_subtask(&task_id, &subtask_id)
            .await
            .expect("should reassign");

        assert!(new_fighter.is_some());
        let new_id = new_fighter.expect("should have new fighter");
        assert_ne!(new_id, original_fighter);
    }

    #[tokio::test]
    async fn test_detect_stale_fighters() {
        let coord = SwarmCoordinator::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        coord.register_fighter(f1);
        coord.register_fighter(f2);

        // Simulate f1 being stale by setting old heartbeat and active tasks.
        if let Some(mut load) = coord.fighter_loads.get_mut(&f1) {
            load.active_tasks = 1;
            load.last_heartbeat = Utc::now().timestamp() - STALE_THRESHOLD_SECS - 10;
        }

        let stale = coord.detect_stale_fighters();
        assert!(stale.contains(&f1));
        assert!(!stale.contains(&f2));

        // f1 should now be unhealthy.
        let load = coord.fighter_loads.get(&f1).expect("should exist");
        assert!(!load.healthy);
    }

    #[test]
    fn test_record_heartbeat() {
        let coord = SwarmCoordinator::new();
        let f = FighterId::new();
        coord.register_fighter(f);

        // Set old heartbeat.
        if let Some(mut load) = coord.fighter_loads.get_mut(&f) {
            load.last_heartbeat = 0;
        }

        coord.record_heartbeat(&f);

        let load = coord.fighter_loads.get(&f).expect("should exist");
        assert!(load.last_heartbeat > 0);
    }

    #[test]
    fn test_list_task_ids() {
        let coord = SwarmCoordinator::new();
        let id1 = coord.create_task("task 1".to_string());
        let id2 = coord.create_task("task 2".to_string());

        let ids = coord.list_task_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }

    #[test]
    fn test_remove_task() {
        let coord = SwarmCoordinator::new();
        let id = coord.create_task("temp".to_string());
        assert!(coord.remove_task(&id));
        assert!(!coord.remove_task(&id)); // Already removed.
    }

    #[tokio::test]
    async fn test_complete_subtask_decrements_load() {
        let coord = SwarmCoordinator::new();
        let f = FighterId::new();
        coord.register_fighter(f);

        let task_id = coord.create_task("work".to_string());
        coord.assign_subtasks(&task_id).await.expect("should assign");

        // Verify load incremented.
        let load = coord.fighter_loads.get(&f).expect("should exist");
        assert_eq!(load.active_tasks, 1);
        drop(load);

        // Get subtask id.
        let task = coord.get_task(&task_id).await.expect("should exist");
        let subtask_id = task.subtasks[0].id;

        coord
            .complete_subtask(&task_id, &subtask_id, "done".to_string())
            .await
            .expect("should complete");

        let load = coord.fighter_loads.get(&f).expect("should exist");
        assert_eq!(load.active_tasks, 0);
    }

    #[tokio::test]
    async fn test_no_fighters_available() {
        let coord = SwarmCoordinator::new();
        let task_id = coord.create_task("lonely task".to_string());
        let assignments = coord.assign_subtasks(&task_id).await.expect("should assign");
        assert!(assignments.is_empty());
    }

    #[tokio::test]
    async fn test_get_nonexistent_task() {
        let coord = SwarmCoordinator::new();
        let result = coord.get_task(&Uuid::new_v4()).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_assign_nonexistent_task() {
        let coord = SwarmCoordinator::new();
        let result = coord.assign_subtasks(&Uuid::new_v4()).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_default_impl() {
        let coord = SwarmCoordinator::default();
        assert_eq!(coord.available_fighter_count(), 0);
    }

    #[tokio::test]
    async fn test_aggregated_result_joins_all() {
        let coord = SwarmCoordinator::new();
        let f = FighterId::new();
        coord.register_fighter(f);

        let task_id = coord.create_task("a\nb\nc".to_string());
        let assignments = coord.assign_subtasks(&task_id).await.expect("assign");

        for (subtask_id, _) in &assignments {
            coord
                .complete_subtask(&task_id, subtask_id, format!("result-{}", subtask_id))
                .await
                .expect("complete");
        }

        let task = coord.get_task(&task_id).await.expect("should exist");
        let agg = task.aggregated_result.expect("should be aggregated");
        assert_eq!(agg.matches("result-").count(), 3);
    }

    #[test]
    fn test_get_fighter_loads() {
        let coord = SwarmCoordinator::new();
        let f1 = FighterId::new();
        let f2 = FighterId::new();
        coord.register_fighter(f1);
        coord.register_fighter(f2);

        let loads = coord.get_fighter_loads();
        assert_eq!(loads.len(), 2);
    }

    #[tokio::test]
    async fn test_capability_aware_assignment() {
        let coord = SwarmCoordinator::new();
        let coder = FighterId::new();
        let reviewer = FighterId::new();
        coord.register_fighter_with_capabilities(coder, vec!["code".to_string()]);
        coord.register_fighter_with_capabilities(reviewer, vec!["review".to_string()]);

        // Create a task about code.
        let subtasks = vec![SwarmSubtask {
            id: Uuid::new_v4(),
            description: "fix the code bug".to_string(),
            assigned_to: None,
            status: SubtaskStatus::Pending,
            result: None,
            depends_on: vec![],
        }];
        let task_id = coord.create_task_with_subtasks("code task".to_string(), subtasks);
        let assignments = coord.assign_subtasks(&task_id).await.expect("should assign");

        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].1, coder);
    }
}
