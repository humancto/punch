//! # punch-kernel
//!
//! **The Ring** — central kernel and orchestrator for the Punch Agent Combat System.
//!
//! This crate coordinates fighters (conversational agents), gorillas (autonomous
//! agents), the event bus, the scheduler, and the agent template registry. It is
//! the single entry point through which the rest of the system interacts with
//! the agent runtime.

pub mod event_bus;
pub mod registry;
pub mod ring;
pub mod scheduler;

pub use event_bus::EventBus;
pub use registry::AgentRegistry;
pub use ring::{FighterEntry, GorillaEntry, Ring};
pub use scheduler::Scheduler;
