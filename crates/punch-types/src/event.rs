use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::fighter::FighterId;
use crate::gorilla::GorillaId;

/// Events emitted by the Punch system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PunchEvent {
    /// A new Fighter has been spawned.
    FighterSpawned { fighter_id: FighterId, name: String },
    /// A Fighter sent or received a message.
    FighterMessage {
        fighter_id: FighterId,
        bout_id: Uuid,
        role: String,
        content_preview: String,
    },
    /// A Gorilla has been unleashed (activated).
    GorillaUnleashed { gorilla_id: GorillaId, name: String },
    /// A Gorilla has been paused.
    GorillaPaused {
        gorilla_id: GorillaId,
        reason: String,
    },
    /// A tool (move) was executed.
    ToolExecuted {
        agent_id: String,
        tool_name: String,
        success: bool,
        duration_ms: u64,
    },
    /// A bout (session/conversation) has started.
    BoutStarted {
        bout_id: Uuid,
        fighter_id: FighterId,
    },
    /// A bout has ended.
    BoutEnded {
        bout_id: Uuid,
        fighter_id: FighterId,
        messages_exchanged: u64,
    },
    /// A combo (workflow) has been triggered.
    ComboTriggered {
        combo_name: String,
        triggered_by: String,
    },
    /// An error occurred in the system.
    Error { source: String, message: String },
}

/// A timestamped event with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventPayload {
    /// Unique identifier for this event instance.
    pub id: Uuid,
    /// When the event occurred.
    pub timestamp: DateTime<Utc>,
    /// The event data.
    pub event: PunchEvent,
    /// Optional correlation ID for tracing related events.
    pub correlation_id: Option<Uuid>,
}

impl EventPayload {
    /// Create a new `EventPayload` with the current timestamp.
    pub fn new(event: PunchEvent) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            event,
            correlation_id: None,
        }
    }

    /// Attach a correlation ID for event tracing.
    pub fn with_correlation(mut self, correlation_id: Uuid) -> Self {
        self.correlation_id = Some(correlation_id);
        self
    }
}
