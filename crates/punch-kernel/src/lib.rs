//! # punch-kernel
//!
//! **The Ring** — central kernel and orchestrator for the Punch Agent Combat System.
//!
//! This crate coordinates fighters (conversational agents), gorillas (autonomous
//! agents), the event bus, the scheduler, the background executor, the workflow
//! engine, and the metering engine. It is the single entry point through which
//! the rest of the system interacts with the agent runtime.

pub mod a2a_executor;
pub mod agent_messaging;
pub mod background;
pub mod budget;
pub mod config_watcher;
pub mod event_bus;
pub mod heartbeat_scheduler;
pub mod metering;
pub mod metrics;
pub mod patterns;
pub mod registry;
pub mod ring;
pub mod scheduler;
pub mod shutdown;
pub mod swarm;
pub mod tenant_registry;
pub mod triggers;
pub mod troop;
pub mod workflow;
pub mod workflow_conditions;
pub mod workflow_loops;
pub mod workflow_validation;

pub use a2a_executor::A2ATaskExecutor;
pub use agent_messaging::MessageRouter;
pub use background::{BackgroundExecutor, fighter_manifest_from_gorilla, run_gorilla_tick};
pub use budget::{BudgetEnforcer, BudgetLimit, BudgetStatus, BudgetVerdict};
pub use config_watcher::{KernelConfigDiff, KernelConfigWatcher};
pub use event_bus::EventBus;
pub use heartbeat_scheduler::{HeartbeatScheduler, HeartbeatStartConfig};
pub use metering::{MeteringEngine, ModelPrice, SpendPeriod};
pub use metrics::{MetricsRegistry, register_default_metrics};
pub use patterns::{
    ChainHandler, MapReduceConfig, MapReduceResult, ScatterResponse, SupervisedWorker,
    SupervisorConfig, auction_filter_bids, auction_select_winner, chain_find_handler, chain_walk,
    execute_auction, execute_chain_of_responsibility, execute_map_reduce,
    execute_map_reduce_distributed, execute_scatter_gather, map_reduce_merge, map_split,
    scatter_select, supervisor_handle_failure, supervisor_monitor_health,
};
pub use registry::AgentRegistry;
pub use ring::{FighterEntry, GorillaEntry, Ring};
pub use scheduler::Scheduler;
pub use shutdown::ShutdownCoordinator;
pub use swarm::{FighterLoad, ProgressReport, SwarmCoordinator};
pub use tenant_registry::TenantRegistry;
pub use triggers::{
    Trigger, TriggerAction, TriggerCondition, TriggerEngine, TriggerId, TriggerSummary,
};
pub use troop::{TaskAssignmentResult, TroopManager};
pub use workflow::{
    CircuitBreakerState, DagExecutionResult, DagWorkflow, DagWorkflowStep, DeadLetterEntry,
    ExecutionTraceEntry, OnError, StepExecutor, StepResult, StepStatus, Workflow, WorkflowEngine,
    WorkflowId, WorkflowRun, WorkflowRunId, WorkflowRunStatus, WorkflowStep, execute_dag,
    expand_dag_variables,
};
pub use workflow_conditions::{Condition, evaluate_condition};
pub use workflow_loops::{LoopConfig, LoopState, calculate_backoff, parse_foreach_items};
pub use workflow_validation::{ValidationError, topological_sort, validate_workflow};
