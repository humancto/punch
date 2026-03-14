//! IPC commands that communicate with the Arena HTTP API.
//!
//! Each command is an async function that calls the Arena REST API via reqwest.
//! These are designed to be directly callable from a Tauri command handler or
//! from the desktop binary's control loop.

use reqwest::Client;
use tracing::{debug, instrument, warn};

use punch_types::{FighterId, PunchResult};

use crate::ipc::{
    ConfigInfo, FighterInfo, GorillaInfo, SendMessageResponse, SpawnFighterResponse, SystemMetrics,
    SystemStatus,
};
use crate::state::DesktopState;

/// Error type for desktop commands.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    /// HTTP request failed.
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    /// Arena returned an error response.
    #[error("arena error ({status}): {message}")]
    Arena { status: u16, message: String },

    /// Deserialization error.
    #[error("deserialization error: {0}")]
    Deserialize(String),

    /// Arena is not connected.
    #[error("arena not connected")]
    NotConnected,
}

/// Result type for desktop commands.
pub type CommandResult<T> = Result<T, CommandError>;

/// Client for issuing commands against the Arena API.
#[derive(Debug, Clone)]
pub struct ArenaClient {
    /// HTTP client.
    client: Client,
    /// Base URL for the Arena API.
    base_url: String,
}

impl ArenaClient {
    /// Create a new Arena client pointing at the given base URL.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
        }
    }

    /// Create an Arena client from desktop state.
    pub fn from_state(state: &DesktopState) -> Self {
        Self::new(&state.arena_url)
    }

    /// Get the base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    // -----------------------------------------------------------------------
    // Fighter commands
    // -----------------------------------------------------------------------

    /// List all active fighters from the Arena API.
    #[instrument(skip(self))]
    pub async fn get_fighters(&self) -> CommandResult<Vec<FighterInfo>> {
        let url = format!("{}/api/fighters", self.base_url);
        debug!(url = %url, "fetching fighters");

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let message = resp.text().await.unwrap_or_default();
            return Err(CommandError::Arena { status, message });
        }

        let fighters: Vec<FighterInfo> = resp
            .json()
            .await
            .map_err(|e| CommandError::Deserialize(e.to_string()))?;

        Ok(fighters)
    }

    /// Spawn a new fighter with the given name and system prompt.
    #[instrument(skip(self))]
    pub async fn spawn_fighter(
        &self,
        name: &str,
        system_prompt: &str,
    ) -> CommandResult<SpawnFighterResponse> {
        let url = format!("{}/api/fighters", self.base_url);
        debug!(url = %url, name = %name, "spawning fighter");

        let body = serde_json::json!({
            "manifest": {
                "name": name,
                "description": format!("Fighter: {name}"),
                "model": {
                    "provider": "anthropic",
                    "model": "claude-sonnet-4-20250514"
                },
                "system_prompt": system_prompt,
                "capabilities": [],
                "weight_class": "middleweight"
            }
        });

        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let message = resp.text().await.unwrap_or_default();
            return Err(CommandError::Arena { status, message });
        }

        let result: SpawnFighterResponse = resp
            .json()
            .await
            .map_err(|e| CommandError::Deserialize(e.to_string()))?;

        Ok(result)
    }

    /// Send a message to a specific fighter.
    #[instrument(skip(self, message))]
    pub async fn send_message(
        &self,
        fighter_id: &FighterId,
        message: &str,
    ) -> CommandResult<SendMessageResponse> {
        let url = format!("{}/api/fighters/{}/message", self.base_url, fighter_id);
        debug!(url = %url, "sending message to fighter");

        let body = serde_json::json!({ "message": message });

        let resp = self.client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let message = resp.text().await.unwrap_or_default();
            return Err(CommandError::Arena { status, message });
        }

        let result: SendMessageResponse = resp
            .json()
            .await
            .map_err(|e| CommandError::Deserialize(e.to_string()))?;

        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Gorilla commands
    // -----------------------------------------------------------------------

    /// List all gorillas from the Arena API.
    #[instrument(skip(self))]
    pub async fn get_gorillas(&self) -> CommandResult<Vec<GorillaInfo>> {
        let url = format!("{}/api/gorillas", self.base_url);
        debug!(url = %url, "fetching gorillas");

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let message = resp.text().await.unwrap_or_default();
            return Err(CommandError::Arena { status, message });
        }

        let gorillas: Vec<GorillaInfo> = resp
            .json()
            .await
            .map_err(|e| CommandError::Deserialize(e.to_string()))?;

        Ok(gorillas)
    }

    // -----------------------------------------------------------------------
    // System commands
    // -----------------------------------------------------------------------

    /// Get system status from the Arena API.
    #[instrument(skip(self))]
    pub async fn get_system_status(&self) -> CommandResult<SystemStatus> {
        let url = format!("{}/api/status", self.base_url);
        debug!(url = %url, "fetching system status");

        let resp = self.client.get(&url).send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let message = resp.text().await.unwrap_or_default();
            return Err(CommandError::Arena { status, message });
        }

        let status: SystemStatus = resp
            .json()
            .await
            .map_err(|e| CommandError::Deserialize(e.to_string()))?;

        Ok(status)
    }

    /// Get the current Punch configuration summary.
    ///
    /// NOTE: The Arena API does not currently expose a /api/config endpoint,
    /// so this returns a default config derived from the connection URL.
    #[instrument(skip(self))]
    pub async fn get_config(&self) -> CommandResult<ConfigInfo> {
        // Try to derive config from the status endpoint and known defaults.
        debug!("building config info from Arena connection");

        let api_listen = self
            .base_url
            .strip_prefix("http://")
            .unwrap_or(&self.base_url)
            .to_string();

        Ok(ConfigInfo {
            api_listen,
            default_provider: "anthropic".to_string(),
            default_model: "claude-sonnet-4-20250514".to_string(),
            auth_enabled: false,
            rate_limit_rpm: 60,
        })
    }

    /// Get system metrics combining Arena data with desktop state.
    #[instrument(skip(self, desktop_state))]
    pub async fn get_metrics(&self, desktop_state: &DesktopState) -> CommandResult<SystemMetrics> {
        debug!("gathering system metrics");

        let arena_status = self.get_system_status().await.ok();

        let (total_fighters, total_gorillas, arena_uptime_secs) =
            if let Some(ref status) = arena_status {
                (
                    status.fighter_count,
                    status.gorilla_count,
                    Some(status.uptime_secs),
                )
            } else {
                (0, 0, None)
            };

        Ok(SystemMetrics {
            total_fighters,
            total_gorillas,
            desktop_uptime_secs: desktop_state.uptime_secs(),
            arena_uptime_secs,
            arena_connected: arena_status.is_some(),
            snapshot_at: chrono::Utc::now(),
        })
    }

    /// Check if the Arena API is reachable.
    #[instrument(skip(self))]
    pub async fn health_check(&self) -> CommandResult<bool> {
        let url = format!("{}/health", self.base_url);
        debug!(url = %url, "health check");

        match self.client.get(&url).send().await {
            Ok(resp) => Ok(resp.status().is_success()),
            Err(e) => {
                warn!(error = %e, "arena health check failed");
                Ok(false)
            }
        }
    }
}

/// Convenience function to check Arena connectivity and update state.
pub async fn check_and_update_connection(
    client: &ArenaClient,
    state: &mut DesktopState,
) -> PunchResult<bool> {
    match client.health_check().await {
        Ok(true) => {
            state.mark_connected();
            Ok(true)
        }
        Ok(false) => {
            state.mark_disconnected();
            Ok(false)
        }
        Err(_) => {
            state.mark_disconnected();
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_arena_client_new() {
        let client = ArenaClient::new("http://localhost:6660");
        assert_eq!(client.base_url(), "http://localhost:6660");
    }

    #[test]
    fn test_arena_client_from_state() {
        let state = DesktopState::new("http://localhost:9999".to_string());
        let client = ArenaClient::from_state(&state);
        assert_eq!(client.base_url(), "http://localhost:9999");
    }

    #[tokio::test]
    async fn test_get_config_returns_defaults() {
        let client = ArenaClient::new("http://localhost:6660");
        let config = client.get_config().await.unwrap();
        assert_eq!(config.api_listen, "localhost:6660");
        assert_eq!(config.default_provider, "anthropic");
        assert_eq!(config.rate_limit_rpm, 60);
    }

    #[tokio::test]
    async fn test_health_check_unreachable() {
        // Connecting to a port that (very likely) has nothing listening.
        let client = ArenaClient::new("http://127.0.0.1:19999");
        let healthy = client.health_check().await.unwrap();
        assert!(!healthy);
    }

    #[tokio::test]
    async fn test_get_fighters_unreachable() {
        let client = ArenaClient::new("http://127.0.0.1:19999");
        let result = client.get_fighters().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_gorillas_unreachable() {
        let client = ArenaClient::new("http://127.0.0.1:19999");
        let result = client.get_gorillas().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_system_status_unreachable() {
        let client = ArenaClient::new("http://127.0.0.1:19999");
        let result = client.get_system_status().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_metrics_with_unreachable_arena() {
        let client = ArenaClient::new("http://127.0.0.1:19999");
        let state = DesktopState::new("http://127.0.0.1:19999".to_string());
        let metrics = client.get_metrics(&state).await.unwrap();
        // When Arena is unreachable, metrics should reflect that.
        assert!(!metrics.arena_connected);
        assert_eq!(metrics.total_fighters, 0);
        assert_eq!(metrics.total_gorillas, 0);
        assert!(metrics.arena_uptime_secs.is_none());
    }

    #[tokio::test]
    async fn test_check_and_update_connection_unreachable() {
        let client = ArenaClient::new("http://127.0.0.1:19999");
        let mut state = DesktopState::new("http://127.0.0.1:19999".to_string());
        state.mark_connected();
        assert!(state.connected);

        let connected = check_and_update_connection(&client, &mut state)
            .await
            .unwrap();
        assert!(!connected);
        assert!(!state.connected);
    }

    #[test]
    fn test_command_error_display() {
        let err = CommandError::NotConnected;
        assert_eq!(err.to_string(), "arena not connected");

        let err = CommandError::Arena {
            status: 404,
            message: "not found".to_string(),
        };
        assert_eq!(err.to_string(), "arena error (404): not found");
    }

    #[test]
    fn test_arena_client_url_construction_fighters() {
        let client = ArenaClient::new("http://localhost:6660");
        // Verify base_url is stored correctly for URL building
        assert_eq!(client.base_url(), "http://localhost:6660");
        // The URL format should be {base_url}/api/fighters
        let expected = format!("{}/api/fighters", client.base_url());
        assert_eq!(expected, "http://localhost:6660/api/fighters");
    }

    #[test]
    fn test_arena_client_url_construction_gorillas() {
        let client = ArenaClient::new("http://localhost:6660");
        let expected = format!("{}/api/gorillas", client.base_url());
        assert_eq!(expected, "http://localhost:6660/api/gorillas");
    }

    #[test]
    fn test_arena_client_url_construction_status() {
        let client = ArenaClient::new("http://localhost:6660");
        let expected = format!("{}/api/status", client.base_url());
        assert_eq!(expected, "http://localhost:6660/api/status");
    }

    #[test]
    fn test_arena_client_url_construction_health() {
        let client = ArenaClient::new("http://localhost:6660");
        let expected = format!("{}/health", client.base_url());
        assert_eq!(expected, "http://localhost:6660/health");
    }

    #[test]
    fn test_arena_client_url_construction_message() {
        let client = ArenaClient::new("http://localhost:6660");
        let id = FighterId(uuid::Uuid::nil());
        let expected = format!("{}/api/fighters/{}/message", client.base_url(), id);
        assert!(expected.contains("/api/fighters/"));
        assert!(expected.contains("/message"));
    }

    #[tokio::test]
    async fn test_get_config_strips_http_prefix() {
        let client = ArenaClient::new("http://127.0.0.1:6660");
        let config = client.get_config().await.unwrap();
        assert_eq!(config.api_listen, "127.0.0.1:6660");
    }

    #[tokio::test]
    async fn test_get_config_handles_no_prefix() {
        let client = ArenaClient::new("localhost:6660");
        let config = client.get_config().await.unwrap();
        assert_eq!(config.api_listen, "localhost:6660");
    }

    #[test]
    fn test_command_error_deserialize() {
        let err = CommandError::Deserialize("invalid JSON".to_string());
        assert_eq!(err.to_string(), "deserialization error: invalid JSON");
    }

    #[tokio::test]
    async fn test_spawn_fighter_unreachable() {
        let client = ArenaClient::new("http://127.0.0.1:19999");
        let result = client.spawn_fighter("test", "You are a test.").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_send_message_unreachable() {
        let client = ArenaClient::new("http://127.0.0.1:19999");
        let id = FighterId(uuid::Uuid::nil());
        let result = client.send_message(&id, "hello").await;
        assert!(result.is_err());
    }
}
