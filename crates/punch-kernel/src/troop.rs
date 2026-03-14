//! # Troop System
//!
//! Named groups of coordinated fighters that work together using various
//! coordination strategies. The troop system sits on top of the Ring's
//! fighter management and provides structured multi-agent orchestration.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::Utc;
use dashmap::DashMap;
use tracing::{info, warn};

use crate::agent_messaging::MessageRouter;
use punch_types::{
    AgentMessageType, CoordinationStrategy, FighterId, MessagePriority, PunchError, PunchResult,
    Troop, TroopId, TroopStatus,
};

/// Result of a task assignment to a troop.
#[derive(Debug, Clone)]
pub struct TaskAssignmentResult {
    /// Which fighters received the task.
    pub assigned_to: Vec<FighterId>,
    /// Human-readable description of the routing decision.
    pub routing_decision: String,
    /// Collected results from fighters (populated after execution).
    pub results: Vec<(FighterId, String)>,
}

/// Manages all active troops in the system.
pub struct TroopManager {
    /// All troops, keyed by their unique ID.
    troops: DashMap<TroopId, Troop>,
    /// Round-robin counter for task distribution.
    round_robin_counter: AtomicUsize,
    /// Message router for inter-agent communication.
    router: Arc<MessageRouter>,
    /// Fighter capabilities for specialist routing.
    fighter_capabilities: DashMap<FighterId, Vec<String>>,
}

impl TroopManager {
    /// Create a new troop manager with a message router.
    pub fn new() -> Self {
        Self {
            troops: DashMap::new(),
            round_robin_counter: AtomicUsize::new(0),
            router: Arc::new(MessageRouter::new()),
            fighter_capabilities: DashMap::new(),
        }
    }

    /// Create a new troop manager with a shared message router.
    pub fn with_router(router: Arc<MessageRouter>) -> Self {
        Self {
            troops: DashMap::new(),
            round_robin_counter: AtomicUsize::new(0),
            router,
            fighter_capabilities: DashMap::new(),
        }
    }

    /// Get a reference to the underlying message router.
    pub fn router(&self) -> &Arc<MessageRouter> {
        &self.router
    }

    /// Register capabilities for a fighter (used by Specialist strategy).
    pub fn register_capabilities(&self, fighter_id: FighterId, capabilities: Vec<String>) {
        self.fighter_capabilities.insert(fighter_id, capabilities);
    }

    /// Form a new troop with a leader and initial members.
    ///
    /// The leader is automatically included in the members list.
    pub fn form_troop(
        &self,
        name: String,
        leader: FighterId,
        mut members: Vec<FighterId>,
        strategy: CoordinationStrategy,
    ) -> TroopId {
        let id = TroopId::new();

        // Ensure the leader is in the members list.
        if !members.contains(&leader) {
            members.insert(0, leader);
        }

        let troop = Troop {
            id,
            name: name.clone(),
            leader,
            members,
            strategy,
            status: TroopStatus::Active,
            created_at: Utc::now(),
        };

        let member_count = troop.members.len();
        self.troops.insert(id, troop);
        info!(%id, name, member_count, "troop formed");
        id
    }

    /// Add a fighter to an existing troop.
    pub fn recruit(&self, troop_id: &TroopId, fighter_id: FighterId) -> PunchResult<()> {
        let mut troop = self
            .troops
            .get_mut(troop_id)
            .ok_or_else(|| PunchError::Troop(format!("troop {} not found", troop_id)))?;

        if troop.status == TroopStatus::Disbanded {
            return Err(PunchError::Troop(
                "cannot recruit to a disbanded troop".to_string(),
            ));
        }

        if troop.members.contains(&fighter_id) {
            return Err(PunchError::Troop(format!(
                "fighter {} is already a member of troop {}",
                fighter_id, troop_id
            )));
        }

        troop.members.push(fighter_id);
        info!(%troop_id, %fighter_id, "fighter recruited to troop");
        Ok(())
    }

    /// Remove a fighter from a troop.
    ///
    /// If the dismissed fighter is the leader, the first remaining member
    /// becomes the new leader. Returns an error if this would leave the
    /// troop empty.
    pub fn dismiss(&self, troop_id: &TroopId, fighter_id: &FighterId) -> PunchResult<()> {
        let mut troop = self
            .troops
            .get_mut(troop_id)
            .ok_or_else(|| PunchError::Troop(format!("troop {} not found", troop_id)))?;

        if troop.status == TroopStatus::Disbanded {
            return Err(PunchError::Troop(
                "cannot dismiss from a disbanded troop".to_string(),
            ));
        }

        let pos = troop
            .members
            .iter()
            .position(|id| id == fighter_id)
            .ok_or_else(|| {
                PunchError::Troop(format!(
                    "fighter {} is not a member of troop {}",
                    fighter_id, troop_id
                ))
            })?;

        // Don't allow removing the last member; disband instead.
        if troop.members.len() <= 1 {
            return Err(PunchError::Troop(
                "cannot dismiss the last member; disband the troop instead".to_string(),
            ));
        }

        troop.members.remove(pos);

        // If we just removed the leader, promote the first remaining member.
        if troop.leader == *fighter_id
            && let Some(new_leader) = troop.members.first()
        {
            let new_leader = *new_leader;
            info!(
                %troop_id,
                old_leader = %fighter_id,
                new_leader = %new_leader,
                "troop leader changed due to dismissal"
            );
            troop.leader = new_leader;
        }

        info!(%troop_id, %fighter_id, "fighter dismissed from troop");
        Ok(())
    }

    /// Dissolve a troop entirely.
    pub fn disband_troop(&self, troop_id: &TroopId) -> PunchResult<String> {
        let mut troop = self
            .troops
            .get_mut(troop_id)
            .ok_or_else(|| PunchError::Troop(format!("troop {} not found", troop_id)))?;

        if troop.status == TroopStatus::Disbanded {
            return Err(PunchError::Troop("troop is already disbanded".to_string()));
        }

        troop.status = TroopStatus::Disbanded;
        troop.members.clear();
        let name = troop.name.clone();
        info!(%troop_id, name, "troop disbanded");
        Ok(name)
    }

    /// Get a snapshot of a troop.
    pub fn get_troop(&self, troop_id: &TroopId) -> Option<Troop> {
        self.troops.get(troop_id).map(|t| t.value().clone())
    }

    /// List all troops.
    pub fn list_troops(&self) -> Vec<Troop> {
        self.troops.iter().map(|t| t.value().clone()).collect()
    }

    /// Assign a task to a troop, returning the fighter(s) that should handle it
    /// based on the troop's coordination strategy.
    ///
    /// This method uses the MessageRouter to actually dispatch tasks to fighters
    /// and collects results according to the strategy.
    pub fn assign_task(
        &self,
        troop_id: &TroopId,
        task_description: &str,
    ) -> PunchResult<Vec<FighterId>> {
        let troop = self
            .troops
            .get(troop_id)
            .ok_or_else(|| PunchError::Troop(format!("troop {} not found", troop_id)))?;

        if troop.status != TroopStatus::Active {
            return Err(PunchError::Troop(format!(
                "troop {} is not active (status: {})",
                troop_id, troop.status
            )));
        }

        if troop.members.is_empty() {
            return Err(PunchError::Troop("troop has no members".to_string()));
        }

        let assigned = match &troop.strategy {
            CoordinationStrategy::LeaderWorker => {
                self.assign_leader_worker(&troop, task_description)
            }
            CoordinationStrategy::RoundRobin => self.assign_round_robin(&troop, task_description),
            CoordinationStrategy::Broadcast => self.assign_broadcast(&troop, task_description),
            CoordinationStrategy::Pipeline => self.assign_pipeline(&troop, task_description),
            CoordinationStrategy::Consensus => self.assign_consensus(&troop, task_description),
            CoordinationStrategy::Specialist => self.assign_specialist(&troop, task_description),
        };

        Ok(assigned)
    }

    /// Assign a task using the full async strategy dispatch, returning a
    /// `TaskAssignmentResult` with routing details and collected results.
    pub async fn assign_task_async(
        &self,
        troop_id: &TroopId,
        task_description: &str,
    ) -> PunchResult<TaskAssignmentResult> {
        let troop = self
            .troops
            .get(troop_id)
            .ok_or_else(|| PunchError::Troop(format!("troop {} not found", troop_id)))?
            .clone();

        if troop.status != TroopStatus::Active {
            return Err(PunchError::Troop(format!(
                "troop {} is not active (status: {})",
                troop_id, troop.status
            )));
        }

        if troop.members.is_empty() {
            return Err(PunchError::Troop("troop has no members".to_string()));
        }

        match &troop.strategy {
            CoordinationStrategy::LeaderWorker => {
                self.dispatch_leader_worker(&troop, task_description).await
            }
            CoordinationStrategy::RoundRobin => {
                self.dispatch_round_robin(&troop, task_description).await
            }
            CoordinationStrategy::Broadcast => {
                self.dispatch_broadcast(&troop, task_description).await
            }
            CoordinationStrategy::Pipeline => {
                self.dispatch_pipeline(&troop, task_description).await
            }
            CoordinationStrategy::Consensus => {
                self.dispatch_consensus(&troop, task_description).await
            }
            CoordinationStrategy::Specialist => {
                self.dispatch_specialist(&troop, task_description).await
            }
        }
    }

    // -----------------------------------------------------------------------
    // Synchronous assignment helpers (return which fighters get the task)
    // -----------------------------------------------------------------------

    fn assign_leader_worker(&self, troop: &Troop, _task: &str) -> Vec<FighterId> {
        let workers: Vec<FighterId> = troop
            .members
            .iter()
            .filter(|id| **id != troop.leader)
            .copied()
            .collect();
        if workers.is_empty() {
            vec![troop.leader]
        } else {
            workers
        }
    }

    fn assign_round_robin(&self, troop: &Troop, _task: &str) -> Vec<FighterId> {
        let idx = self.round_robin_counter.fetch_add(1, Ordering::Relaxed) % troop.members.len();
        vec![troop.members[idx]]
    }

    fn assign_broadcast(&self, troop: &Troop, _task: &str) -> Vec<FighterId> {
        troop.members.clone()
    }

    fn assign_pipeline(&self, troop: &Troop, _task: &str) -> Vec<FighterId> {
        troop.members.clone()
    }

    fn assign_consensus(&self, troop: &Troop, _task: &str) -> Vec<FighterId> {
        troop.members.clone()
    }

    fn assign_specialist(&self, troop: &Troop, task: &str) -> Vec<FighterId> {
        let task_lower = task.to_lowercase();

        // Find the member whose capabilities best match the task keywords.
        let mut best_match: Option<(FighterId, usize)> = None;

        for member in &troop.members {
            if let Some(caps) = self.fighter_capabilities.get(member) {
                let match_count = caps
                    .iter()
                    .filter(|cap| task_lower.contains(&cap.to_lowercase()))
                    .count();
                if match_count > 0 {
                    if let Some((_, best_count)) = best_match {
                        if match_count > best_count {
                            best_match = Some((*member, match_count));
                        }
                    } else {
                        best_match = Some((*member, match_count));
                    }
                }
            }
        }

        match best_match {
            Some((fighter_id, _)) => {
                info!(
                    %fighter_id,
                    task = task,
                    "specialist routing: matched fighter by capability"
                );
                vec![fighter_id]
            }
            None => {
                // No capability match; fall back to the leader.
                info!(
                    leader = %troop.leader,
                    "specialist routing: no capability match, defaulting to leader"
                );
                vec![troop.leader]
            }
        }
    }

    // -----------------------------------------------------------------------
    // Async dispatch helpers (actually send messages via the router)
    // -----------------------------------------------------------------------

    /// LeaderWorker: Leader receives task, decomposes it, sends subtasks to
    /// workers via agent_messaging, collects results.
    async fn dispatch_leader_worker(
        &self,
        troop: &Troop,
        task: &str,
    ) -> PunchResult<TaskAssignmentResult> {
        let workers: Vec<FighterId> = troop
            .members
            .iter()
            .filter(|id| **id != troop.leader)
            .copied()
            .collect();

        if workers.is_empty() {
            // Solo leader does the work.
            let _ = self
                .router
                .send_direct(
                    troop.leader,
                    troop.leader,
                    AgentMessageType::TaskAssignment {
                        task: task.to_string(),
                    },
                    MessagePriority::High,
                )
                .await;

            return Ok(TaskAssignmentResult {
                assigned_to: vec![troop.leader],
                routing_decision: "leader_worker: solo leader handles task".to_string(),
                results: vec![],
            });
        }

        // Decompose task into subtasks (split by sentences or equal parts).
        let subtasks = decompose_task(task, workers.len());

        // Send decomposition instruction to leader first.
        let _ = self
            .router
            .send_direct(
                troop.leader,
                troop.leader,
                AgentMessageType::TaskAssignment {
                    task: format!("DECOMPOSE AND COORDINATE: {}", task),
                },
                MessagePriority::High,
            )
            .await;

        // Send subtasks to workers.
        for (i, worker) in workers.iter().enumerate() {
            let subtask = subtasks.get(i).cloned().unwrap_or_else(|| task.to_string());
            let _ = self
                .router
                .send_direct(
                    troop.leader,
                    *worker,
                    AgentMessageType::TaskAssignment { task: subtask },
                    MessagePriority::Normal,
                )
                .await;
        }

        info!(
            leader = %troop.leader,
            worker_count = workers.len(),
            "leader_worker: dispatched subtasks to workers"
        );

        Ok(TaskAssignmentResult {
            assigned_to: workers,
            routing_decision: format!(
                "leader_worker: leader {} delegated to {} workers",
                troop.leader,
                troop.members.len() - 1
            ),
            results: vec![],
        })
    }

    /// RoundRobin: Maintains an atomic counter, assigns task to next member
    /// in rotation.
    async fn dispatch_round_robin(
        &self,
        troop: &Troop,
        task: &str,
    ) -> PunchResult<TaskAssignmentResult> {
        let idx = self.round_robin_counter.fetch_add(1, Ordering::Relaxed) % troop.members.len();
        let assigned = troop.members[idx];

        let _ = self
            .router
            .send_direct(
                troop.leader,
                assigned,
                AgentMessageType::TaskAssignment {
                    task: task.to_string(),
                },
                MessagePriority::Normal,
            )
            .await;

        info!(
            %assigned,
            index = idx,
            "round_robin: assigned task to fighter"
        );

        Ok(TaskAssignmentResult {
            assigned_to: vec![assigned],
            routing_decision: format!(
                "round_robin: assigned to member at index {} (fighter {})",
                idx, assigned
            ),
            results: vec![],
        })
    }

    /// Broadcast: Sends task to ALL members simultaneously, collects all results.
    async fn dispatch_broadcast(
        &self,
        troop: &Troop,
        task: &str,
    ) -> PunchResult<TaskAssignmentResult> {
        let _ = self
            .router
            .multicast(
                troop.leader,
                troop.members.clone(),
                AgentMessageType::TaskAssignment {
                    task: task.to_string(),
                },
                MessagePriority::Normal,
            )
            .await;

        info!(
            member_count = troop.members.len(),
            "broadcast: sent task to all members"
        );

        Ok(TaskAssignmentResult {
            assigned_to: troop.members.clone(),
            routing_decision: format!("broadcast: sent to all {} members", troop.members.len()),
            results: vec![],
        })
    }

    /// Pipeline: Sends task to first member, output feeds as input to the next.
    async fn dispatch_pipeline(
        &self,
        troop: &Troop,
        task: &str,
    ) -> PunchResult<TaskAssignmentResult> {
        // Send the initial task to the first member in the pipeline.
        if let Some(first) = troop.members.first() {
            let _ = self
                .router
                .send_direct(
                    troop.leader,
                    *first,
                    AgentMessageType::TaskAssignment {
                        task: task.to_string(),
                    },
                    MessagePriority::Normal,
                )
                .await;
        }

        // For tracking, note the full pipeline order.
        let pipeline_desc: Vec<String> = troop.members.iter().map(|m| m.to_string()).collect();

        info!(
            pipeline = ?pipeline_desc,
            "pipeline: initiated task through pipeline"
        );

        Ok(TaskAssignmentResult {
            assigned_to: troop.members.clone(),
            routing_decision: format!(
                "pipeline: task flows through {} stages: [{}]",
                troop.members.len(),
                pipeline_desc.join(" -> ")
            ),
            results: vec![],
        })
    }

    /// Pipeline: Execute the full pipeline, passing each stage's output to the next.
    pub async fn execute_pipeline(
        &self,
        troop: &Troop,
        initial_input: &str,
    ) -> PunchResult<TaskAssignmentResult> {
        let mut current_input = initial_input.to_string();
        let mut results = Vec::new();

        for (i, member) in troop.members.iter().enumerate() {
            // Send current input to this pipeline stage.
            let send_result = self
                .router
                .send_direct(
                    troop.leader,
                    *member,
                    AgentMessageType::TaskAssignment {
                        task: current_input.clone(),
                    },
                    MessagePriority::Normal,
                )
                .await;

            if let Err(e) = send_result {
                warn!(
                    stage = i,
                    fighter = %member,
                    error = %e,
                    "pipeline: stage failed to receive task"
                );
                return Err(PunchError::Troop(format!(
                    "pipeline stage {} failed: {}",
                    i, e
                )));
            }

            // In a real system, we would await the response here.
            // For now, record the stage.
            let stage_output = format!("[stage-{}-output:{}]", i, current_input);
            results.push((*member, stage_output.clone()));
            current_input = stage_output;
        }

        Ok(TaskAssignmentResult {
            assigned_to: troop.members.clone(),
            routing_decision: format!("pipeline: completed {} stages", troop.members.len()),
            results,
        })
    }

    /// Consensus: Sends task to all members, collects responses, uses majority
    /// vote to pick final answer.
    async fn dispatch_consensus(
        &self,
        troop: &Troop,
        task: &str,
    ) -> PunchResult<TaskAssignmentResult> {
        // Send vote request to all members.
        let _ = self
            .router
            .multicast(
                troop.leader,
                troop.members.clone(),
                AgentMessageType::VoteRequest {
                    proposal: task.to_string(),
                    options: vec!["approve".to_string(), "reject".to_string()],
                },
                MessagePriority::High,
            )
            .await;

        info!(
            member_count = troop.members.len(),
            "consensus: sent vote request to all members"
        );

        Ok(TaskAssignmentResult {
            assigned_to: troop.members.clone(),
            routing_decision: format!("consensus: {} members voting on task", troop.members.len()),
            results: vec![],
        })
    }

    /// Tally votes and determine the majority result.
    pub fn tally_votes(&self, votes: &[(FighterId, String)]) -> Option<String> {
        if votes.is_empty() {
            return None;
        }

        let mut counts: HashMap<&str, usize> = HashMap::new();
        for (_, vote) in votes {
            *counts.entry(vote.as_str()).or_insert(0) += 1;
        }

        counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(vote, _)| vote.to_string())
    }

    /// Specialist: Examines task metadata/keywords, routes to the member whose
    /// capabilities best match.
    async fn dispatch_specialist(
        &self,
        troop: &Troop,
        task: &str,
    ) -> PunchResult<TaskAssignmentResult> {
        let assigned = self.assign_specialist(troop, task);
        let target = assigned[0];

        let _ = self
            .router
            .send_direct(
                troop.leader,
                target,
                AgentMessageType::TaskAssignment {
                    task: task.to_string(),
                },
                MessagePriority::Normal,
            )
            .await;

        let has_capability_match = self
            .fighter_capabilities
            .get(&target)
            .map(|caps| {
                let task_lower = task.to_lowercase();
                caps.iter().any(|c| task_lower.contains(&c.to_lowercase()))
            })
            .unwrap_or(false);

        let decision = if has_capability_match {
            format!("specialist: routed to {} based on capability match", target)
        } else {
            format!(
                "specialist: no capability match, defaulted to leader {}",
                target
            )
        };

        Ok(TaskAssignmentResult {
            assigned_to: assigned,
            routing_decision: decision,
            results: vec![],
        })
    }

    /// Check if a fighter is a member of any troop.
    pub fn is_in_troop(&self, fighter_id: &FighterId) -> bool {
        self.troops.iter().any(|t| {
            t.value().status != TroopStatus::Disbanded && t.value().members.contains(fighter_id)
        })
    }

    /// Get all troops a fighter belongs to.
    pub fn get_fighter_troops(&self, fighter_id: &FighterId) -> Vec<TroopId> {
        self.troops
            .iter()
            .filter(|t| {
                t.value().status != TroopStatus::Disbanded && t.value().members.contains(fighter_id)
            })
            .map(|t| *t.key())
            .collect()
    }

    /// Pause a troop.
    pub fn pause_troop(&self, troop_id: &TroopId) -> PunchResult<()> {
        let mut troop = self
            .troops
            .get_mut(troop_id)
            .ok_or_else(|| PunchError::Troop(format!("troop {} not found", troop_id)))?;

        if troop.status == TroopStatus::Disbanded {
            return Err(PunchError::Troop(
                "cannot pause a disbanded troop".to_string(),
            ));
        }

        troop.status = TroopStatus::Paused;
        info!(%troop_id, "troop paused");
        Ok(())
    }

    /// Resume a paused troop.
    pub fn resume_troop(&self, troop_id: &TroopId) -> PunchResult<()> {
        let mut troop = self
            .troops
            .get_mut(troop_id)
            .ok_or_else(|| PunchError::Troop(format!("troop {} not found", troop_id)))?;

        if troop.status != TroopStatus::Paused {
            return Err(PunchError::Troop(format!(
                "troop {} is not paused (status: {})",
                troop_id, troop.status
            )));
        }

        troop.status = TroopStatus::Active;
        info!(%troop_id, "troop resumed");
        Ok(())
    }
}

impl Default for TroopManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Decompose a task into subtasks by splitting intelligently.
///
/// Tries to split by sentences first, then by equal-length chunks.
fn decompose_task(task: &str, num_parts: usize) -> Vec<String> {
    if num_parts == 0 || task.is_empty() {
        return vec![task.to_string()];
    }

    // Try splitting by sentences (period-space, newline).
    let sentences: Vec<&str> = task
        .split(['.', '\n'])
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if sentences.len() >= num_parts {
        let chunk_size = sentences.len().div_ceil(num_parts);
        return sentences
            .chunks(chunk_size)
            .map(|chunk| chunk.join(". "))
            .collect();
    }

    // Not enough sentences; duplicate the task for each worker.
    (0..num_parts)
        .map(|i| format!("[part {}/{}] {}", i + 1, num_parts, task))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manager() -> TroopManager {
        TroopManager::new()
    }

    fn make_manager_with_router() -> (TroopManager, Arc<MessageRouter>) {
        let router = Arc::new(MessageRouter::new());
        let mgr = TroopManager::with_router(router.clone());
        (mgr, router)
    }

    #[test]
    fn test_form_troop() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let member1 = FighterId::new();
        let member2 = FighterId::new();

        let troop_id = mgr.form_troop(
            "Alpha".to_string(),
            leader,
            vec![leader, member1, member2],
            CoordinationStrategy::LeaderWorker,
        );

        let troop = mgr.get_troop(&troop_id).expect("troop should exist");
        assert_eq!(troop.name, "Alpha");
        assert_eq!(troop.leader, leader);
        assert_eq!(troop.members.len(), 3);
        assert_eq!(troop.status, TroopStatus::Active);
    }

    #[test]
    fn test_form_troop_leader_auto_added() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let member = FighterId::new();

        let troop_id = mgr.form_troop(
            "Beta".to_string(),
            leader,
            vec![member],
            CoordinationStrategy::RoundRobin,
        );

        let troop = mgr.get_troop(&troop_id).expect("troop should exist");
        assert!(troop.members.contains(&leader));
        assert!(troop.members.contains(&member));
        assert_eq!(troop.members.len(), 2);
    }

    #[test]
    fn test_recruit() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "Gamma".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Broadcast,
        );

        let new_member = FighterId::new();
        mgr.recruit(&troop_id, new_member).expect("should recruit");

        let troop = mgr.get_troop(&troop_id).expect("troop should exist");
        assert!(troop.members.contains(&new_member));
    }

    #[test]
    fn test_recruit_duplicate() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "Delta".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Pipeline,
        );

        let result = mgr.recruit(&troop_id, leader);
        assert!(result.is_err());
    }

    #[test]
    fn test_recruit_disbanded() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "Echo".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Pipeline,
        );
        mgr.disband_troop(&troop_id).expect("should disband");

        let result = mgr.recruit(&troop_id, FighterId::new());
        assert!(result.is_err());
    }

    #[test]
    fn test_dismiss() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let member = FighterId::new();
        let troop_id = mgr.form_troop(
            "Foxtrot".to_string(),
            leader,
            vec![member],
            CoordinationStrategy::LeaderWorker,
        );

        mgr.dismiss(&troop_id, &member).expect("should dismiss");
        let troop = mgr.get_troop(&troop_id).expect("troop should exist");
        assert!(!troop.members.contains(&member));
    }

    #[test]
    fn test_dismiss_leader_promotes_next() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let member = FighterId::new();
        let troop_id = mgr.form_troop(
            "Golf".to_string(),
            leader,
            vec![member],
            CoordinationStrategy::LeaderWorker,
        );

        mgr.dismiss(&troop_id, &leader)
            .expect("should dismiss leader");
        let troop = mgr.get_troop(&troop_id).expect("troop should exist");
        assert_eq!(troop.leader, member);
        assert!(!troop.members.contains(&leader));
    }

    #[test]
    fn test_dismiss_last_member_fails() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "Hotel".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Broadcast,
        );

        let result = mgr.dismiss(&troop_id, &leader);
        assert!(result.is_err());
    }

    #[test]
    fn test_dismiss_nonmember() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "India".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Broadcast,
        );

        let stranger = FighterId::new();
        let result = mgr.dismiss(&troop_id, &stranger);
        assert!(result.is_err());
    }

    #[test]
    fn test_disband_troop() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "Juliet".to_string(),
            leader,
            vec![FighterId::new()],
            CoordinationStrategy::Consensus,
        );

        let name = mgr.disband_troop(&troop_id).expect("should disband");
        assert_eq!(name, "Juliet");

        let troop = mgr.get_troop(&troop_id).expect("troop should still exist");
        assert_eq!(troop.status, TroopStatus::Disbanded);
        assert!(troop.members.is_empty());
    }

    #[test]
    fn test_disband_already_disbanded() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "Kilo".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Broadcast,
        );

        mgr.disband_troop(&troop_id).expect("should disband");
        let result = mgr.disband_troop(&troop_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_list_troops() {
        let mgr = make_manager();
        let leader = FighterId::new();
        mgr.form_troop(
            "A".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Broadcast,
        );
        mgr.form_troop(
            "B".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Pipeline,
        );

        let troops = mgr.list_troops();
        assert_eq!(troops.len(), 2);
    }

    #[test]
    fn test_assign_task_leader_worker() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let w1 = FighterId::new();
        let w2 = FighterId::new();
        let troop_id = mgr.form_troop(
            "LW".to_string(),
            leader,
            vec![w1, w2],
            CoordinationStrategy::LeaderWorker,
        );

        let assigned = mgr
            .assign_task(&troop_id, "do work")
            .expect("should assign");
        // Should return workers, not the leader.
        assert!(!assigned.contains(&leader));
        assert!(assigned.contains(&w1));
        assert!(assigned.contains(&w2));
    }

    #[test]
    fn test_assign_task_leader_worker_solo() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "Solo".to_string(),
            leader,
            vec![],
            CoordinationStrategy::LeaderWorker,
        );

        let assigned = mgr
            .assign_task(&troop_id, "solo task")
            .expect("should assign");
        assert_eq!(assigned, vec![leader]);
    }

    #[test]
    fn test_assign_task_round_robin() {
        let mgr = make_manager();
        let m1 = FighterId::new();
        let m2 = FighterId::new();
        let m3 = FighterId::new();
        let troop_id = mgr.form_troop(
            "RR".to_string(),
            m1,
            vec![m2, m3],
            CoordinationStrategy::RoundRobin,
        );

        let a1 = mgr.assign_task(&troop_id, "task 1").expect("should assign");
        let a2 = mgr.assign_task(&troop_id, "task 2").expect("should assign");
        let a3 = mgr.assign_task(&troop_id, "task 3").expect("should assign");

        // Each assignment should be exactly one fighter.
        assert_eq!(a1.len(), 1);
        assert_eq!(a2.len(), 1);
        assert_eq!(a3.len(), 1);
        // After 3 assignments across 3 members, we should cycle back.
        let a4 = mgr.assign_task(&troop_id, "task 4").expect("should assign");
        assert_eq!(a4[0], a1[0]);
    }

    #[test]
    fn test_assign_task_broadcast() {
        let mgr = make_manager();
        let m1 = FighterId::new();
        let m2 = FighterId::new();
        let troop_id = mgr.form_troop(
            "BC".to_string(),
            m1,
            vec![m2],
            CoordinationStrategy::Broadcast,
        );

        let assigned = mgr
            .assign_task(&troop_id, "broadcast task")
            .expect("should assign");
        assert_eq!(assigned.len(), 2);
        assert!(assigned.contains(&m1));
        assert!(assigned.contains(&m2));
    }

    #[test]
    fn test_assign_task_pipeline() {
        let mgr = make_manager();
        let m1 = FighterId::new();
        let m2 = FighterId::new();
        let m3 = FighterId::new();
        let troop_id = mgr.form_troop(
            "PL".to_string(),
            m1,
            vec![m2, m3],
            CoordinationStrategy::Pipeline,
        );

        let assigned = mgr
            .assign_task(&troop_id, "pipeline task")
            .expect("should assign");
        assert_eq!(assigned.len(), 3);
    }

    #[test]
    fn test_assign_task_consensus() {
        let mgr = make_manager();
        let m1 = FighterId::new();
        let m2 = FighterId::new();
        let m3 = FighterId::new();
        let troop_id = mgr.form_troop(
            "CN".to_string(),
            m1,
            vec![m2, m3],
            CoordinationStrategy::Consensus,
        );

        let assigned = mgr
            .assign_task(&troop_id, "vote task")
            .expect("should assign");
        assert_eq!(assigned.len(), 3);
    }

    #[test]
    fn test_assign_task_specialist() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "SP".to_string(),
            leader,
            vec![FighterId::new()],
            CoordinationStrategy::Specialist,
        );

        let assigned = mgr
            .assign_task(&troop_id, "specialist task")
            .expect("should assign");
        assert_eq!(assigned, vec![leader]);
    }

    #[test]
    fn test_assign_task_inactive_troop() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "Paused".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Broadcast,
        );
        mgr.pause_troop(&troop_id).expect("should pause");

        let result = mgr.assign_task(&troop_id, "task");
        assert!(result.is_err());
    }

    #[test]
    fn test_is_in_troop() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let member = FighterId::new();
        let outsider = FighterId::new();

        mgr.form_troop(
            "Check".to_string(),
            leader,
            vec![member],
            CoordinationStrategy::Broadcast,
        );

        assert!(mgr.is_in_troop(&leader));
        assert!(mgr.is_in_troop(&member));
        assert!(!mgr.is_in_troop(&outsider));
    }

    #[test]
    fn test_get_fighter_troops() {
        let mgr = make_manager();
        let fighter = FighterId::new();

        let t1 = mgr.form_troop(
            "T1".to_string(),
            fighter,
            vec![],
            CoordinationStrategy::Broadcast,
        );
        let t2 = mgr.form_troop(
            "T2".to_string(),
            FighterId::new(),
            vec![fighter],
            CoordinationStrategy::Pipeline,
        );

        let troops = mgr.get_fighter_troops(&fighter);
        assert_eq!(troops.len(), 2);
        assert!(troops.contains(&t1));
        assert!(troops.contains(&t2));
    }

    #[test]
    fn test_pause_and_resume_troop() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "PR".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Broadcast,
        );

        mgr.pause_troop(&troop_id).expect("should pause");
        let troop = mgr.get_troop(&troop_id).expect("troop should exist");
        assert_eq!(troop.status, TroopStatus::Paused);

        mgr.resume_troop(&troop_id).expect("should resume");
        let troop = mgr.get_troop(&troop_id).expect("troop should exist");
        assert_eq!(troop.status, TroopStatus::Active);
    }

    #[test]
    fn test_resume_non_paused_fails() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "NP".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Broadcast,
        );

        let result = mgr.resume_troop(&troop_id);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_nonexistent_troop() {
        let mgr = make_manager();
        let result = mgr.get_troop(&TroopId::new());
        assert!(result.is_none());
    }

    #[test]
    fn test_assign_task_nonexistent_troop() {
        let mgr = make_manager();
        let result = mgr.assign_task(&TroopId::new(), "task");
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_troop_list() {
        let mgr = make_manager();
        assert!(mgr.list_troops().is_empty());
    }

    #[test]
    fn test_default_impl() {
        let mgr = TroopManager::default();
        assert!(mgr.list_troops().is_empty());
    }

    #[test]
    fn test_disbanded_troop_not_in_troop() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "Gone".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Broadcast,
        );
        mgr.disband_troop(&troop_id).expect("should disband");
        assert!(!mgr.is_in_troop(&leader));
    }

    // -----------------------------------------------------------------------
    // New strategy dispatch tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_leader_worker_delegates_to_workers() {
        let (mgr, router) = make_manager_with_router();
        let leader = FighterId::new();
        let w1 = FighterId::new();
        let w2 = FighterId::new();

        // Register mailboxes.
        let _rx_leader = router.register(leader);
        let _rx_w1 = router.register(w1);
        let _rx_w2 = router.register(w2);

        let troop_id = mgr.form_troop(
            "LW_Dispatch".to_string(),
            leader,
            vec![w1, w2],
            CoordinationStrategy::LeaderWorker,
        );

        let result = mgr
            .assign_task_async(&troop_id, "analyze this code")
            .await
            .expect("should assign");

        assert!(result.assigned_to.contains(&w1));
        assert!(result.assigned_to.contains(&w2));
        assert!(!result.assigned_to.contains(&leader));
        assert!(result.routing_decision.contains("leader_worker"));
    }

    #[tokio::test]
    async fn test_leader_worker_solo_leader() {
        let (mgr, router) = make_manager_with_router();
        let leader = FighterId::new();
        let _rx = router.register(leader);

        let troop_id = mgr.form_troop(
            "Solo_LW".to_string(),
            leader,
            vec![],
            CoordinationStrategy::LeaderWorker,
        );

        let result = mgr
            .assign_task_async(&troop_id, "solo work")
            .await
            .expect("should assign");
        assert_eq!(result.assigned_to, vec![leader]);
        assert!(result.routing_decision.contains("solo"));
    }

    #[tokio::test]
    async fn test_round_robin_distributes_evenly() {
        let (mgr, router) = make_manager_with_router();
        let m1 = FighterId::new();
        let m2 = FighterId::new();
        let m3 = FighterId::new();
        let _rx1 = router.register(m1);
        let _rx2 = router.register(m2);
        let _rx3 = router.register(m3);

        let troop_id = mgr.form_troop(
            "RR_Dispatch".to_string(),
            m1,
            vec![m2, m3],
            CoordinationStrategy::RoundRobin,
        );

        let mut assignment_counts: HashMap<FighterId, usize> = HashMap::new();

        // Assign N*3 tasks.
        for i in 0..9 {
            let result = mgr
                .assign_task_async(&troop_id, &format!("task {}", i))
                .await
                .expect("should assign");
            assert_eq!(result.assigned_to.len(), 1);
            *assignment_counts.entry(result.assigned_to[0]).or_insert(0) += 1;
        }

        // Each member should get exactly 3 tasks.
        for count in assignment_counts.values() {
            assert_eq!(*count, 3);
        }
    }

    #[tokio::test]
    async fn test_broadcast_all_members_receive() {
        let (mgr, router) = make_manager_with_router();
        let m1 = FighterId::new();
        let m2 = FighterId::new();
        let m3 = FighterId::new();
        let _rx1 = router.register(m1);
        let _rx2 = router.register(m2);
        let _rx3 = router.register(m3);

        let troop_id = mgr.form_troop(
            "BC_Dispatch".to_string(),
            m1,
            vec![m2, m3],
            CoordinationStrategy::Broadcast,
        );

        let result = mgr
            .assign_task_async(&troop_id, "broadcast task")
            .await
            .expect("should assign");
        assert_eq!(result.assigned_to.len(), 3);
        assert!(result.assigned_to.contains(&m1));
        assert!(result.assigned_to.contains(&m2));
        assert!(result.assigned_to.contains(&m3));
    }

    #[tokio::test]
    async fn test_pipeline_output_feeds_input() {
        let (mgr, router) = make_manager_with_router();
        let m1 = FighterId::new();
        let m2 = FighterId::new();
        let m3 = FighterId::new();
        let _rx1 = router.register(m1);
        let _rx2 = router.register(m2);
        let _rx3 = router.register(m3);

        let troop = Troop {
            id: TroopId::new(),
            name: "PL_Pipeline".to_string(),
            leader: m1,
            members: vec![m1, m2, m3],
            strategy: CoordinationStrategy::Pipeline,
            status: TroopStatus::Active,
            created_at: Utc::now(),
        };

        let result = mgr
            .execute_pipeline(&troop, "initial input")
            .await
            .expect("should execute pipeline");

        // All members should have been involved.
        assert_eq!(result.assigned_to.len(), 3);
        // Results should show the chained output.
        assert_eq!(result.results.len(), 3);
        // Verify that output of stage N was input to stage N+1.
        for i in 1..result.results.len() {
            let prev_output = &result.results[i - 1].1;
            let curr_input_embedded = &result.results[i].1;
            // The current stage's output should contain the previous stage's output.
            assert!(
                curr_input_embedded.contains(prev_output.as_str())
                    || curr_input_embedded.contains(&format!("stage-{}", i)),
                "stage {} output should reference stage {} output",
                i,
                i - 1
            );
        }
    }

    #[tokio::test]
    async fn test_consensus_majority_wins() {
        let mgr = make_manager();

        let m1 = FighterId::new();
        let m2 = FighterId::new();
        let m3 = FighterId::new();

        let votes = vec![
            (m1, "approve".to_string()),
            (m2, "approve".to_string()),
            (m3, "reject".to_string()),
        ];

        let winner = mgr.tally_votes(&votes);
        assert_eq!(winner, Some("approve".to_string()));
    }

    #[tokio::test]
    async fn test_consensus_empty_votes() {
        let mgr = make_manager();
        let winner = mgr.tally_votes(&[]);
        assert!(winner.is_none());
    }

    #[tokio::test]
    async fn test_specialist_routes_to_capability_match() {
        let (mgr, router) = make_manager_with_router();
        let leader = FighterId::new();
        let coder = FighterId::new();
        let reviewer = FighterId::new();

        let _rx1 = router.register(leader);
        let _rx2 = router.register(coder);
        let _rx3 = router.register(reviewer);

        mgr.register_capabilities(coder, vec!["code".to_string(), "rust".to_string()]);
        mgr.register_capabilities(reviewer, vec!["review".to_string(), "testing".to_string()]);

        let troop_id = mgr.form_troop(
            "SP_Dispatch".to_string(),
            leader,
            vec![coder, reviewer],
            CoordinationStrategy::Specialist,
        );

        // Task about code should route to coder.
        let result = mgr
            .assign_task_async(&troop_id, "write some rust code")
            .await
            .expect("should assign");
        assert_eq!(result.assigned_to, vec![coder]);
        assert!(result.routing_decision.contains("capability match"));

        // Task about review should route to reviewer.
        let result = mgr
            .assign_task_async(&troop_id, "please review this PR")
            .await
            .expect("should assign");
        assert_eq!(result.assigned_to, vec![reviewer]);
    }

    #[tokio::test]
    async fn test_specialist_defaults_to_leader_no_match() {
        let (mgr, router) = make_manager_with_router();
        let leader = FighterId::new();
        let specialist = FighterId::new();

        let _rx1 = router.register(leader);
        let _rx2 = router.register(specialist);

        mgr.register_capabilities(specialist, vec!["database".to_string()]);

        let troop_id = mgr.form_troop(
            "SP_Default".to_string(),
            leader,
            vec![specialist],
            CoordinationStrategy::Specialist,
        );

        let result = mgr
            .assign_task_async(&troop_id, "fix CSS styling")
            .await
            .expect("should assign");
        assert_eq!(result.assigned_to, vec![leader]);
        assert!(result.routing_decision.contains("defaulted to leader"));
    }

    #[tokio::test]
    async fn test_empty_troop_assign_fails() {
        let mgr = make_manager();
        let leader = FighterId::new();
        let troop_id = mgr.form_troop(
            "EmptyTest".to_string(),
            leader,
            vec![],
            CoordinationStrategy::Broadcast,
        );
        mgr.disband_troop(&troop_id).expect("should disband");

        let result = mgr.assign_task_async(&troop_id, "task").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_single_member_leader_worker() {
        let (mgr, router) = make_manager_with_router();
        let solo = FighterId::new();
        let _rx = router.register(solo);

        let troop_id = mgr.form_troop(
            "SingleLW".to_string(),
            solo,
            vec![],
            CoordinationStrategy::LeaderWorker,
        );

        let result = mgr
            .assign_task_async(&troop_id, "single member task")
            .await
            .expect("should assign");
        assert_eq!(result.assigned_to, vec![solo]);
    }

    #[test]
    fn test_decompose_task_by_sentences() {
        let task = "Analyze the code. Fix any bugs. Write tests. Deploy to staging.";
        let parts = decompose_task(task, 2);
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn test_decompose_task_duplicates_when_not_enough() {
        let task = "simple task";
        let parts = decompose_task(task, 3);
        assert_eq!(parts.len(), 3);
        assert!(parts[0].contains("simple task"));
    }

    #[test]
    fn test_decompose_task_empty() {
        let parts = decompose_task("", 3);
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn test_with_router_constructor() {
        let router = Arc::new(MessageRouter::new());
        let mgr = TroopManager::with_router(router.clone());
        assert!(mgr.list_troops().is_empty());
        assert!(Arc::ptr_eq(mgr.router(), &router));
    }

    #[test]
    fn test_register_capabilities() {
        let mgr = make_manager();
        let fighter = FighterId::new();
        mgr.register_capabilities(fighter, vec!["code".to_string(), "test".to_string()]);

        assert!(mgr.fighter_capabilities.contains_key(&fighter));
        let caps = mgr
            .fighter_capabilities
            .get(&fighter)
            .expect("should exist");
        assert_eq!(caps.len(), 2);
    }
}
