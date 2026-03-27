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
    /// Images produced during the bout (e.g. screenshots, generated images).
    /// Each entry is a base64-encoded image with its media type.
    #[serde(default)]
    pub images: Vec<ResponseImage>,
}

/// An image produced during a bout, ready to be sent to a channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseImage {
    /// Base64-encoded image data.
    pub data: String,
    /// MIME type (e.g. "image/png", "image/jpeg").
    pub media_type: String,
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

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_agent_info_serde_roundtrip() {
        let info = AgentInfo {
            id: FighterId(Uuid::nil()),
            name: "TestAgent".to_string(),
            status: FighterStatus::Idle,
        };
        let json = serde_json::to_string(&info).expect("serialize");
        let deser: AgentInfo = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.name, "TestAgent");
        assert_eq!(deser.status, FighterStatus::Idle);
    }

    #[test]
    fn test_agent_info_all_statuses() {
        let statuses = vec![
            FighterStatus::Idle,
            FighterStatus::Fighting,
            FighterStatus::Resting,
            FighterStatus::KnockedOut,
            FighterStatus::Training,
        ];
        for status in statuses {
            let info = AgentInfo {
                id: FighterId::new(),
                name: "Agent".to_string(),
                status,
            };
            let json = serde_json::to_string(&info).expect("serialize");
            let deser: AgentInfo = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(deser.status, status);
        }
    }

    #[test]
    fn test_agent_message_result_serde() {
        let result = AgentMessageResult {
            response: "I processed your request".to_string(),
            tokens_used: 256,
            images: vec![],
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let deser: AgentMessageResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.response, "I processed your request");
        assert_eq!(deser.tokens_used, 256);
    }

    #[test]
    fn test_agent_message_result_empty_response() {
        let result = AgentMessageResult {
            response: String::new(),
            tokens_used: 0,
            images: vec![],
        };
        let json = serde_json::to_string(&result).expect("serialize");
        let deser: AgentMessageResult = serde_json::from_str(&json).expect("deserialize");
        assert!(deser.response.is_empty());
        assert_eq!(deser.tokens_used, 0);
    }

    #[test]
    fn test_agent_info_clone() {
        let info = AgentInfo {
            id: FighterId(Uuid::nil()),
            name: "Cloneable".to_string(),
            status: FighterStatus::Fighting,
        };
        let cloned = info.clone();
        assert_eq!(cloned.name, info.name);
        assert_eq!(cloned.status, info.status);
    }
}
