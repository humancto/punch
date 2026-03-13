//! IPC message types for desktop ↔ frontend communication.
//!
//! These types define the protocol for future Tauri/webview integration.
//! Currently used by the commands module as return types.

use chrono::{DateTime, Utc};
use punch_types::{FighterId, FighterStatus, GorillaId, GorillaStatus, WeightClass};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Fighter IPC types
// ---------------------------------------------------------------------------

/// Summary of a fighter for list views.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FighterInfo {
    /// Unique identifier.
    pub id: FighterId,
    /// Human-readable name.
    pub name: String,
    /// Description of the fighter's purpose.
    pub description: String,
    /// Weight class / capability tier.
    pub weight_class: WeightClass,
    /// Current operational status.
    pub status: FighterStatus,
}

/// Request to spawn a new fighter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnFighterRequest {
    /// Name for the new fighter.
    pub name: String,
    /// System prompt defining the fighter's behavior.
    pub system_prompt: String,
}

/// Response from spawning a fighter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnFighterResponse {
    /// Assigned fighter ID.
    pub id: FighterId,
    /// Name echoed back.
    pub name: String,
}

/// Request to send a message to a fighter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    /// Target fighter ID.
    pub fighter_id: FighterId,
    /// The message text.
    pub message: String,
}

/// Response from sending a message to a fighter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageResponse {
    /// The fighter's reply.
    pub response: String,
    /// Tokens consumed.
    pub tokens_used: u64,
}

// ---------------------------------------------------------------------------
// Gorilla IPC types
// ---------------------------------------------------------------------------

/// Summary of a gorilla for list views.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GorillaInfo {
    /// Unique identifier.
    pub id: GorillaId,
    /// Human-readable name.
    pub name: String,
    /// Description of the gorilla's purpose.
    pub description: String,
    /// Cron-style schedule.
    pub schedule: String,
    /// Current operational status.
    pub status: GorillaStatus,
}

// ---------------------------------------------------------------------------
// System IPC types
// ---------------------------------------------------------------------------

/// System-wide status information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    /// Overall system health.
    pub status: String,
    /// Number of active fighters.
    pub fighter_count: usize,
    /// Number of registered gorillas.
    pub gorilla_count: usize,
    /// Uptime in seconds.
    pub uptime_secs: i64,
    /// Approximate memory usage in bytes (from OS, if available).
    pub memory_bytes: Option<u64>,
}

/// System metrics snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    /// Total fighters spawned since startup.
    pub total_fighters: usize,
    /// Total gorillas registered.
    pub total_gorillas: usize,
    /// Desktop app uptime in seconds.
    pub desktop_uptime_secs: i64,
    /// Arena API uptime in seconds (if connected).
    pub arena_uptime_secs: Option<i64>,
    /// Whether the Arena connection is healthy.
    pub arena_connected: bool,
    /// Timestamp of this snapshot.
    pub snapshot_at: DateTime<Utc>,
}

/// Configuration summary returned to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigInfo {
    /// Arena API listen address.
    pub api_listen: String,
    /// Default model provider.
    pub default_provider: String,
    /// Default model name.
    pub default_model: String,
    /// Whether authentication is enabled.
    pub auth_enabled: bool,
    /// Rate limit in requests per minute.
    pub rate_limit_rpm: u32,
}

// ---------------------------------------------------------------------------
// IPC envelope (for future use with Tauri custom protocol)
// ---------------------------------------------------------------------------

/// Envelope for IPC messages sent from frontend to backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    /// Command name (e.g., "get_fighters", "spawn_fighter").
    pub command: String,
    /// JSON-encoded payload.
    pub payload: serde_json::Value,
}

/// Envelope for IPC responses sent from backend to frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    /// Whether the command succeeded.
    pub success: bool,
    /// JSON-encoded result data (on success).
    pub data: Option<serde_json::Value>,
    /// Error message (on failure).
    pub error: Option<String>,
}

impl IpcResponse {
    /// Create a successful response.
    pub fn ok(data: serde_json::Value) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    /// Create an error response.
    pub fn err(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ipc_response_ok() {
        let resp = IpcResponse::ok(serde_json::json!({"count": 5}));
        assert!(resp.success);
        assert!(resp.data.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_ipc_response_err() {
        let resp = IpcResponse::err("something went wrong");
        assert!(!resp.success);
        assert!(resp.data.is_none());
        assert_eq!(resp.error.as_deref(), Some("something went wrong"));
    }

    #[test]
    fn test_ipc_request_serialization() {
        let req = IpcRequest {
            command: "get_fighters".to_string(),
            payload: serde_json::json!({}),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: IpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.command, "get_fighters");
    }

    #[test]
    fn test_fighter_info_serialization() {
        let info = FighterInfo {
            id: FighterId(uuid::Uuid::nil()),
            name: "TestFighter".to_string(),
            description: "A test fighter".to_string(),
            weight_class: WeightClass::Middleweight,
            status: FighterStatus::Idle,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("TestFighter"));
        let deserialized: FighterInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "TestFighter");
    }

    #[test]
    fn test_system_status_serialization() {
        let status = SystemStatus {
            status: "ok".to_string(),
            fighter_count: 3,
            gorilla_count: 1,
            uptime_secs: 120,
            memory_bytes: Some(1024 * 1024),
        };
        let json = serde_json::to_string(&status).unwrap();
        let deserialized: SystemStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.fighter_count, 3);
        assert_eq!(deserialized.memory_bytes, Some(1024 * 1024));
    }

    #[test]
    fn test_gorilla_info_serialization() {
        let info = GorillaInfo {
            id: GorillaId(uuid::Uuid::nil()),
            name: "TestGorilla".to_string(),
            description: "A test gorilla".to_string(),
            schedule: "*/5 * * * *".to_string(),
            status: GorillaStatus::Caged,
        };
        let json = serde_json::to_string(&info).unwrap();
        let deserialized: GorillaInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "TestGorilla");
        assert_eq!(deserialized.schedule, "*/5 * * * *");
    }

    #[test]
    fn test_config_info_serialization() {
        let config = ConfigInfo {
            api_listen: "0.0.0.0:6660".to_string(),
            default_provider: "anthropic".to_string(),
            default_model: "claude-sonnet-4-20250514".to_string(),
            auth_enabled: true,
            rate_limit_rpm: 60,
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: ConfigInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.api_listen, "0.0.0.0:6660");
        assert!(deserialized.auth_enabled);
    }

    #[test]
    fn test_system_metrics_serialization() {
        let metrics = SystemMetrics {
            total_fighters: 5,
            total_gorillas: 2,
            desktop_uptime_secs: 300,
            arena_uptime_secs: Some(250),
            arena_connected: true,
            snapshot_at: Utc::now(),
        };
        let json = serde_json::to_string(&metrics).unwrap();
        let deserialized: SystemMetrics = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.total_fighters, 5);
        assert!(deserialized.arena_connected);
    }

    #[test]
    fn test_spawn_fighter_request() {
        let req = SpawnFighterRequest {
            name: "Boxer".to_string(),
            system_prompt: "You are a boxer.".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: SpawnFighterRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "Boxer");
    }

    #[test]
    fn test_send_message_request() {
        let req = SendMessageRequest {
            fighter_id: FighterId(uuid::Uuid::nil()),
            message: "Hello fighter".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: SendMessageRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.message, "Hello fighter");
    }
}
