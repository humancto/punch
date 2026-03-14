//! # punch-memory
//!
//! Persistence layer for the Punch Agent Combat System.
//!
//! All storage is backed by SQLite via `rusqlite`. The [`MemorySubstrate`]
//! wraps a connection behind a `tokio::sync::Mutex` so it can be shared
//! safely across async tasks.

pub mod backup;
pub mod bouts;
pub mod channels;
pub mod consolidation;
pub mod creed;
pub mod embeddings;
pub mod fighters;
pub mod knowledge;
pub mod maintenance;
pub mod memories;
pub mod migrations;
pub mod substrate;
pub mod usage;

pub use backup::{BackupInfo, BackupManager};
pub use bouts::{BoutId, BoutSummary};
pub use channels::ChannelRecord;
pub use consolidation::{ConsolidationConfig, ConsolidationResult, MemoryConsolidator};
pub use embeddings::{
    BuiltInEmbedder, Embedder, Embedding, EmbeddingConfig, EmbeddingProvider, EmbeddingStore,
    OpenAiEmbedder, cosine_similarity, top_k_similar,
};
pub use knowledge::{KnowledgeEntity, KnowledgeRelation};
pub use memories::MemoryEntry;
pub use migrations::{Migration, MigrationEngine, MigrationStatus};
pub use substrate::MemorySubstrate;
pub use usage::{UsageEvent, UsageSummary};
