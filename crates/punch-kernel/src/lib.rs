//! # punch-kernel
//!
//! **The Ring** — central kernel and orchestrator for the Punch Agent Combat System.
//!
//! This crate coordinates fighters (conversational agents), gorillas (autonomous
//! agents), the event bus, the scheduler, the background executor, the workflow
//! engine, and the metering engine. It is the single entry point through which
//! the rest of the system interacts with the agent runtime.

pub mod a2a_executor;
pub mod background;
pub mod event_bus;
pub mod metering;
pub mod registry;
pub mod ring;
pub mod scheduler;
pub mod triggers;
pub mod workflow;

pub use a2a_executor::A2ATaskExecutor;
pub use background::{BackgroundExecutor, fighter_manifest_from_gorilla, run_gorilla_tick};
pub use event_bus::EventBus;
pub use metering::{MeteringEngine, ModelPrice, SpendPeriod};
pub use registry::AgentRegistry;
pub use ring::{FighterEntry, GorillaEntry, Ring};
pub use scheduler::Scheduler;
pub use triggers::{
    Trigger, TriggerAction, TriggerCondition, TriggerEngine, TriggerId, TriggerSummary,
};
pub use workflow::{
    OnError, StepResult, Workflow, WorkflowEngine, WorkflowId, WorkflowRun, WorkflowRunId,
    WorkflowRunStatus, WorkflowStep,
};
