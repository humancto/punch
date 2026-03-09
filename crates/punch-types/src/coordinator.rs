//! Agent coordination trait for inter-agent messaging.
//!
//! This trait is defined in `punch-types` so that `punch-runtime` can use it
//! without depending on `punch-kernel`. The `Ring` in `punch-kernel` provides
//! the concrete implementation.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PunchResult;
use crate::fighter::{FighterId, FighterManifest, FighterStatus};

/// Summary information about a fighter, returned by agent coordination calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    /// The fighter's unique ID.
    pub id: FighterId,
    /// Human-readable name.
    pub name: String,
    /// Current status.
    pub status: FighterStatus,
}

/// The result of sending a message to another agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessageResult {
    /// The response text from the target agent.
    pub response: String,
    /// Tokens consumed by the target agent's processing.
    pub tokens_used: u64,
}

/// Trait for coordinating inter-agent operations.
///
/// This allows the tool executor in `punch-runtime` to spawn fighters,
/// send messages, and list agents without depending on `punch-kernel`.
/// The `Ring` implements this trait and is passed as `Arc<dyn AgentCoordinator>`
/// into the tool execution context.
#[async_trait]
pub trait AgentCoordinator: Send + Sync {
    /// Spawn a new fighter from a manifest.
    ///
    /// Returns the newly assigned fighter ID.
    async fn spawn_fighter(&self, manifest: FighterManifest) -> PunchResult<FighterId>;

    /// Send a message to a fighter and get its response.
    ///
    /// This creates a nested agent call: the calling fighter's tool execution
    /// invokes the target fighter's agent loop.
    async fn send_message_to_agent(
        &self,
        target: &FighterId,
        message: String,
    ) -> PunchResult<AgentMessageResult>;

    /// Find a fighter by name.
    ///
    /// Returns the fighter ID if found.
    async fn find_fighter_by_name(&self, name: &str) -> PunchResult<Option<FighterId>>;

    /// List all active fighters.
    async fn list_fighters(&self) -> PunchResult<Vec<AgentInfo>>;
}
