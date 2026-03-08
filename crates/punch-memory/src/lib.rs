//! # punch-memory
//!
//! Persistence layer for the Punch Agent Combat System.
//!
//! All storage is backed by SQLite via `rusqlite`. The [`MemorySubstrate`]
//! wraps a connection behind a `tokio::sync::Mutex` so it can be shared
//! safely across async tasks.

pub mod bouts;
pub mod fighters;
pub mod knowledge;
pub mod memories;
pub mod migrations;
pub mod substrate;
pub mod usage;

pub use bouts::{BoutId, BoutSummary};
pub use knowledge::{KnowledgeEntity, KnowledgeRelation};
pub use memories::MemoryEntry;
pub use substrate::MemorySubstrate;
pub use usage::{UsageEvent, UsageSummary};
