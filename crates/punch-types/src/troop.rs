//! # Troop Types
//!
//! Shared types for the multi-agent troop coordination system.
//! Troops are named groups of coordinated fighters (agents) that
//! work together using various coordination strategies.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::fighter::FighterId;

/// Unique identifier for a Troop (coordinated agent group).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TroopId(pub Uuid);

impl TroopId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TroopId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TroopId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Coordination strategy determining how a troop distributes work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinationStrategy {
    /// Leader delegates tasks, workers execute and report back.
    LeaderWorker,
    /// Tasks distributed evenly across members.
    RoundRobin,
    /// All members receive same task, results aggregated.
    Broadcast,
    /// Each member processes output of previous member.
    Pipeline,
    /// Members vote on decisions, majority wins.
    Consensus,
    /// Tasks routed to member with matching capabilities.
    Specialist,
}

impl std::fmt::Display for CoordinationStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LeaderWorker => write!(f, "leader_worker"),
            Self::RoundRobin => write!(f, "round_robin"),
            Self::Broadcast => write!(f, "broadcast"),
            Self::Pipeline => write!(f, "pipeline"),
            Self::Consensus => write!(f, "consensus"),
            Self::Specialist => write!(f, "specialist"),
        }
    }
}

/// Current operational status of a Troop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TroopStatus {
    /// Troop is being assembled, not yet operational.
    Forming,
    /// Troop is active and ready to receive tasks.
    Active,
    /// Troop is temporarily paused.
    Paused,
    /// Troop has been dissolved.
    Disbanded,
}

impl std::fmt::Display for TroopStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Forming => write!(f, "forming"),
            Self::Active => write!(f, "active"),
            Self::Paused => write!(f, "paused"),
            Self::Disbanded => write!(f, "disbanded"),
        }
    }
}

/// A named group of coordinated agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Troop {
    /// Unique identifier for this troop.
    pub id: TroopId,
    /// Human-readable name for this troop.
    pub name: String,
    /// The leader fighter who coordinates the troop.
    pub leader: FighterId,
    /// All member fighters (including the leader).
    pub members: Vec<FighterId>,
    /// How tasks are distributed among members.
    pub strategy: CoordinationStrategy,
    /// Current operational status.
    pub status: TroopStatus,
    /// When the troop was formed.
    pub created_at: DateTime<Utc>,
}

/// Status of a single subtask within a swarm task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtaskStatus {
    /// Waiting to be assigned.
    Pending,
    /// Assigned to a fighter and running.
    Running,
    /// Completed successfully.
    Completed,
    /// Failed with an error.
    Failed(String),
}

/// A subtask within a larger swarm task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmSubtask {
    /// Unique identifier for this subtask.
    pub id: Uuid,
    /// Description of what this subtask should accomplish.
    pub description: String,
    /// The fighter assigned to this subtask, if any.
    pub assigned_to: Option<FighterId>,
    /// Current status.
    pub status: SubtaskStatus,
    /// Result content, if completed.
    pub result: Option<String>,
    /// Dependencies: IDs of subtasks that must complete first.
    pub depends_on: Vec<Uuid>,
}

/// A complex task decomposed into subtasks for swarm execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmTask {
    /// Unique identifier.
    pub id: Uuid,
    /// The original task description.
    pub description: String,
    /// Decomposed subtasks.
    pub subtasks: Vec<SwarmSubtask>,
    /// Overall progress (0.0 to 1.0).
    pub progress: f64,
    /// When the swarm task was created.
    pub created_at: DateTime<Utc>,
    /// Aggregated result, if all subtasks completed.
    pub aggregated_result: Option<String>,
}

/// Priority levels for inter-agent messages.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MessagePriority {
    /// Low priority, can be deferred.
    Low,
    /// Normal priority (default).
    #[default]
    Normal,
    /// High priority, process promptly.
    High,
    /// Critical, process immediately.
    Critical,
}

/// Types of channels for inter-agent messaging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageChannel {
    /// Point-to-point between two fighters.
    Direct,
    /// One-to-all in a troop.
    Broadcast,
    /// One-to-some (subset of troop).
    Multicast(Vec<FighterId>),
    /// Send and wait for response (with timeout in milliseconds).
    Request { timeout_ms: u64 },
    /// Continuous data flow between agents.
    Stream,
}

/// Types of messages exchanged between agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum AgentMessageType {
    /// Assign work to a fighter.
    TaskAssignment { task: String },
    /// Report task completion.
    TaskResult { result: String, success: bool },
    /// Heartbeat / progress update.
    StatusUpdate { progress: f64, detail: String },
    /// Share context/knowledge between agents.
    DataShare {
        key: String,
        value: serde_json::Value,
    },
    /// Request a vote from peers.
    VoteRequest {
        proposal: String,
        options: Vec<String>,
    },
    /// Respond to a vote request.
    VoteResponse { proposal: String, vote: String },
    /// Task escalation to leader.
    Escalation {
        reason: String,
        original_task: String,
    },
}

/// An inter-agent message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    /// Unique message ID.
    pub id: Uuid,
    /// The sending fighter.
    pub from: FighterId,
    /// The receiving fighter.
    pub to: FighterId,
    /// Channel type.
    pub channel: MessageChannel,
    /// Message content.
    pub content: AgentMessageType,
    /// Priority level.
    pub priority: MessagePriority,
    /// When the message was sent.
    pub timestamp: DateTime<Utc>,
    /// Whether the message has been delivered.
    pub delivered: bool,
}

/// Restart strategy for the Supervisor pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartStrategy {
    /// Only restart the failed worker.
    OneForOne,
    /// Restart all workers if one fails.
    AllForOne,
}

/// A bid from an agent in the Auction pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuctionBid {
    /// The bidding fighter.
    pub fighter_id: FighterId,
    /// Estimated time to complete (in seconds).
    pub estimated_time_secs: u64,
    /// Confidence level (0.0 to 1.0).
    pub confidence: f64,
    /// When the bid was submitted.
    pub submitted_at: DateTime<Utc>,
}

/// Selection criteria for the Scatter-Gather pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionCriteria {
    /// Select the fastest response.
    Fastest,
    /// Select based on highest reported quality.
    HighestQuality,
    /// Select based on consensus among responses.
    Consensus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_troop_id_display() {
        let uuid = Uuid::nil();
        let id = TroopId(uuid);
        assert_eq!(id.to_string(), uuid.to_string());
    }

    #[test]
    fn test_troop_id_new_is_unique() {
        let id1 = TroopId::new();
        let id2 = TroopId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_troop_id_default() {
        let id = TroopId::default();
        assert_ne!(id.0, Uuid::nil());
    }

    #[test]
    fn test_troop_id_serde_transparent() {
        let uuid = Uuid::new_v4();
        let id = TroopId(uuid);
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{}\"", uuid));
        let deser: TroopId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser, id);
    }

    #[test]
    fn test_troop_id_copy_clone() {
        let id = TroopId::new();
        let copied = id;
        let cloned = id.clone();
        assert_eq!(id, copied);
        assert_eq!(id, cloned);
    }

    #[test]
    fn test_troop_id_hash() {
        let id = TroopId::new();
        let mut set = std::collections::HashSet::new();
        set.insert(id);
        set.insert(id);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_coordination_strategy_display() {
        assert_eq!(
            CoordinationStrategy::LeaderWorker.to_string(),
            "leader_worker"
        );
        assert_eq!(CoordinationStrategy::RoundRobin.to_string(), "round_robin");
        assert_eq!(CoordinationStrategy::Broadcast.to_string(), "broadcast");
        assert_eq!(CoordinationStrategy::Pipeline.to_string(), "pipeline");
        assert_eq!(CoordinationStrategy::Consensus.to_string(), "consensus");
        assert_eq!(CoordinationStrategy::Specialist.to_string(), "specialist");
    }

    #[test]
    fn test_coordination_strategy_serde_roundtrip() {
        let strategies = vec![
            CoordinationStrategy::LeaderWorker,
            CoordinationStrategy::RoundRobin,
            CoordinationStrategy::Broadcast,
            CoordinationStrategy::Pipeline,
            CoordinationStrategy::Consensus,
            CoordinationStrategy::Specialist,
        ];
        for strategy in &strategies {
            let json = serde_json::to_string(strategy).expect("serialize");
            let deser: CoordinationStrategy = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, strategy);
        }
    }

    #[test]
    fn test_troop_status_display() {
        assert_eq!(TroopStatus::Forming.to_string(), "forming");
        assert_eq!(TroopStatus::Active.to_string(), "active");
        assert_eq!(TroopStatus::Paused.to_string(), "paused");
        assert_eq!(TroopStatus::Disbanded.to_string(), "disbanded");
    }

    #[test]
    fn test_troop_status_serde_roundtrip() {
        let statuses = vec![
            TroopStatus::Forming,
            TroopStatus::Active,
            TroopStatus::Paused,
            TroopStatus::Disbanded,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).expect("serialize");
            let deser: TroopStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, status);
        }
    }

    #[test]
    fn test_troop_serde_roundtrip() {
        let troop = Troop {
            id: TroopId::new(),
            name: "Alpha Squad".to_string(),
            leader: FighterId::new(),
            members: vec![FighterId::new(), FighterId::new()],
            strategy: CoordinationStrategy::LeaderWorker,
            status: TroopStatus::Active,
            created_at: Utc::now(),
        };
        let json = serde_json::to_string(&troop).expect("serialize");
        let deser: Troop = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.id, troop.id);
        assert_eq!(deser.name, "Alpha Squad");
        assert_eq!(deser.members.len(), 2);
    }

    #[test]
    fn test_message_priority_default() {
        assert_eq!(MessagePriority::default(), MessagePriority::Normal);
    }

    #[test]
    fn test_message_priority_ordering() {
        assert!(MessagePriority::Low < MessagePriority::Normal);
        assert!(MessagePriority::Normal < MessagePriority::High);
        assert!(MessagePriority::High < MessagePriority::Critical);
    }

    #[test]
    fn test_message_channel_serde() {
        let channels = vec![
            MessageChannel::Direct,
            MessageChannel::Broadcast,
            MessageChannel::Multicast(vec![FighterId::new()]),
            MessageChannel::Request { timeout_ms: 5000 },
            MessageChannel::Stream,
        ];
        for channel in &channels {
            let json = serde_json::to_string(channel).expect("serialize");
            let deser: MessageChannel = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, channel);
        }
    }

    #[test]
    fn test_agent_message_type_task_assignment() {
        let msg = AgentMessageType::TaskAssignment {
            task: "analyze code".to_string(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        assert!(json.contains("task_assignment"));
        let deser: AgentMessageType = serde_json::from_str(&json).expect("deserialize");
        match deser {
            AgentMessageType::TaskAssignment { task } => {
                assert_eq!(task, "analyze code");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_agent_message_type_vote_request() {
        let msg = AgentMessageType::VoteRequest {
            proposal: "merge PR?".to_string(),
            options: vec!["yes".to_string(), "no".to_string()],
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: AgentMessageType = serde_json::from_str(&json).expect("deserialize");
        match deser {
            AgentMessageType::VoteRequest { proposal, options } => {
                assert_eq!(proposal, "merge PR?");
                assert_eq!(options.len(), 2);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_agent_message_serde() {
        let msg = AgentMessage {
            id: Uuid::new_v4(),
            from: FighterId::new(),
            to: FighterId::new(),
            channel: MessageChannel::Direct,
            content: AgentMessageType::StatusUpdate {
                progress: 0.5,
                detail: "halfway done".to_string(),
            },
            priority: MessagePriority::Normal,
            timestamp: Utc::now(),
            delivered: false,
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: AgentMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.id, msg.id);
        assert!(!deser.delivered);
    }

    #[test]
    fn test_subtask_status_serde() {
        let statuses = vec![
            SubtaskStatus::Pending,
            SubtaskStatus::Running,
            SubtaskStatus::Completed,
            SubtaskStatus::Failed("error".to_string()),
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).expect("serialize");
            let deser: SubtaskStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, status);
        }
    }

    #[test]
    fn test_swarm_task_progress() {
        let task = SwarmTask {
            id: Uuid::new_v4(),
            description: "big task".to_string(),
            subtasks: vec![],
            progress: 0.75,
            created_at: Utc::now(),
            aggregated_result: None,
        };
        assert!((task.progress - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn test_auction_bid_serde() {
        let bid = AuctionBid {
            fighter_id: FighterId::new(),
            estimated_time_secs: 30,
            confidence: 0.9,
            submitted_at: Utc::now(),
        };
        let json = serde_json::to_string(&bid).expect("serialize");
        let deser: AuctionBid = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.estimated_time_secs, 30);
        assert!((deser.confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn test_selection_criteria_serde() {
        let criteria = vec![
            SelectionCriteria::Fastest,
            SelectionCriteria::HighestQuality,
            SelectionCriteria::Consensus,
        ];
        for c in &criteria {
            let json = serde_json::to_string(c).expect("serialize");
            let deser: SelectionCriteria = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, c);
        }
    }

    #[test]
    fn test_restart_strategy_serde() {
        let strategies = vec![RestartStrategy::OneForOne, RestartStrategy::AllForOne];
        for s in &strategies {
            let json = serde_json::to_string(s).expect("serialize");
            let deser: RestartStrategy = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(&deser, s);
        }
    }

    #[test]
    fn test_escalation_message() {
        let msg = AgentMessageType::Escalation {
            reason: "too complex".to_string(),
            original_task: "analyze codebase".to_string(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: AgentMessageType = serde_json::from_str(&json).expect("deserialize");
        match deser {
            AgentMessageType::Escalation {
                reason,
                original_task,
            } => {
                assert_eq!(reason, "too complex");
                assert_eq!(original_task, "analyze codebase");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_data_share_message() {
        let msg = AgentMessageType::DataShare {
            key: "context".to_string(),
            value: serde_json::json!({"files": ["main.rs"]}),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: AgentMessageType = serde_json::from_str(&json).expect("deserialize");
        match deser {
            AgentMessageType::DataShare { key, value } => {
                assert_eq!(key, "context");
                assert!(value.get("files").is_some());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_swarm_subtask_with_dependencies() {
        let dep_id = Uuid::new_v4();
        let subtask = SwarmSubtask {
            id: Uuid::new_v4(),
            description: "step 2".to_string(),
            assigned_to: Some(FighterId::new()),
            status: SubtaskStatus::Pending,
            result: None,
            depends_on: vec![dep_id],
        };
        let json = serde_json::to_string(&subtask).expect("serialize");
        let deser: SwarmSubtask = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.depends_on.len(), 1);
        assert_eq!(deser.depends_on[0], dep_id);
    }
}
