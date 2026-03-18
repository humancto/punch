use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::fighter::FighterId;
use crate::gorilla::GorillaId;
use crate::troop::TroopId;

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
    /// A troop has been formed.
    TroopFormed {
        troop_id: TroopId,
        name: String,
        member_count: usize,
    },
    /// A troop has been disbanded.
    TroopDisbanded { troop_id: TroopId, name: String },
    /// An MCP server has been started and initialized.
    McpServerStarted { server_name: String },
    /// An MCP server has been shut down.
    McpServerStopped { server_name: String },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fighter_spawned_serde() {
        let event = PunchEvent::FighterSpawned {
            fighter_id: FighterId(Uuid::nil()),
            name: "TestFighter".to_string(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"kind\":\"fighter_spawned\""));
        let deser: PunchEvent = serde_json::from_str(&json).expect("deserialize");
        match deser {
            PunchEvent::FighterSpawned { name, .. } => assert_eq!(name, "TestFighter"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_fighter_message_serde() {
        let event = PunchEvent::FighterMessage {
            fighter_id: FighterId(Uuid::nil()),
            bout_id: Uuid::nil(),
            role: "user".to_string(),
            content_preview: "Hello".to_string(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"kind\":\"fighter_message\""));
        let deser: PunchEvent = serde_json::from_str(&json).expect("deserialize");
        match deser {
            PunchEvent::FighterMessage {
                content_preview, ..
            } => assert_eq!(content_preview, "Hello"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_gorilla_unleashed_serde() {
        let event = PunchEvent::GorillaUnleashed {
            gorilla_id: GorillaId(Uuid::nil()),
            name: "AlphaGorilla".to_string(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"kind\":\"gorilla_unleashed\""));
        let deser: PunchEvent = serde_json::from_str(&json).expect("deserialize");
        match deser {
            PunchEvent::GorillaUnleashed { name, .. } => assert_eq!(name, "AlphaGorilla"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_tool_executed_serde() {
        let event = PunchEvent::ToolExecuted {
            agent_id: "agent-1".to_string(),
            tool_name: "web_fetch".to_string(),
            success: true,
            duration_ms: 150,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let deser: PunchEvent = serde_json::from_str(&json).expect("deserialize");
        match deser {
            PunchEvent::ToolExecuted {
                success,
                duration_ms,
                ..
            } => {
                assert!(success);
                assert_eq!(duration_ms, 150);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_bout_started_serde() {
        let event = PunchEvent::BoutStarted {
            bout_id: Uuid::nil(),
            fighter_id: FighterId(Uuid::nil()),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"kind\":\"bout_started\""));
    }

    #[test]
    fn test_bout_ended_serde() {
        let event = PunchEvent::BoutEnded {
            bout_id: Uuid::nil(),
            fighter_id: FighterId(Uuid::nil()),
            messages_exchanged: 42,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let deser: PunchEvent = serde_json::from_str(&json).expect("deserialize");
        match deser {
            PunchEvent::BoutEnded {
                messages_exchanged, ..
            } => assert_eq!(messages_exchanged, 42),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_combo_triggered_serde() {
        let event = PunchEvent::ComboTriggered {
            combo_name: "deploy-pipeline".to_string(),
            triggered_by: "admin".to_string(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"kind\":\"combo_triggered\""));
    }

    #[test]
    fn test_error_event_serde() {
        let event = PunchEvent::Error {
            source: "kernel".to_string(),
            message: "out of memory".to_string(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        let deser: PunchEvent = serde_json::from_str(&json).expect("deserialize");
        match deser {
            PunchEvent::Error { source, message } => {
                assert_eq!(source, "kernel");
                assert_eq!(message, "out of memory");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_event_payload_new() {
        let event = PunchEvent::FighterSpawned {
            fighter_id: FighterId(Uuid::nil()),
            name: "Test".to_string(),
        };
        let payload = EventPayload::new(event);
        assert!(payload.correlation_id.is_none());
        assert!(payload.timestamp <= Utc::now());
    }

    #[test]
    fn test_event_payload_with_correlation() {
        let event = PunchEvent::Error {
            source: "test".to_string(),
            message: "msg".to_string(),
        };
        let corr_id = Uuid::new_v4();
        let payload = EventPayload::new(event).with_correlation(corr_id);
        assert_eq!(payload.correlation_id, Some(corr_id));
    }

    #[test]
    fn test_event_payload_serde_roundtrip() {
        let event = PunchEvent::ToolExecuted {
            agent_id: "a1".to_string(),
            tool_name: "read_file".to_string(),
            success: false,
            duration_ms: 0,
        };
        let payload = EventPayload::new(event);
        let json = serde_json::to_string(&payload).expect("serialize");
        let deser: EventPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.id, payload.id);
    }

    #[test]
    fn test_gorilla_paused_serde() {
        let event = PunchEvent::GorillaPaused {
            gorilla_id: GorillaId(Uuid::nil()),
            reason: "rate limited".to_string(),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"kind\":\"gorilla_paused\""));
        let deser: PunchEvent = serde_json::from_str(&json).expect("deserialize");
        match deser {
            PunchEvent::GorillaPaused { reason, .. } => assert_eq!(reason, "rate limited"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_event_clone() {
        let event = PunchEvent::FighterSpawned {
            fighter_id: FighterId(Uuid::nil()),
            name: "Clone".to_string(),
        };
        let cloned = event.clone();
        let json1 = serde_json::to_string(&event).unwrap();
        let json2 = serde_json::to_string(&cloned).unwrap();
        assert_eq!(json1, json2);
    }
}
