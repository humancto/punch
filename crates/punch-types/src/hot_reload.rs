//! Hot Config Reload — corner team adjustments between rounds.
//!
//! This module enables mid-fight strategy changes by watching the config file
//! for modifications and broadcasting validated updates to all subscribers.
//! Like a corner team making tactical adjustments between rounds, the system
//! applies config changes without pulling the fighter from the ring.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, watch};
use tracing::{debug, error, info, warn};

use crate::config::PunchConfig;
use crate::error::{PunchError, PunchResult};

/// Watches the config file and broadcasts validated changes — the corner team
/// that keeps the fighter's strategy sharp without stopping the bout.
#[derive(Debug)]
pub struct ConfigWatcher {
    /// Path to the configuration file being watched.
    config_path: PathBuf,
    /// Thread-safe handle to the current configuration.
    current: Arc<RwLock<PunchConfig>>,
    /// Sender half of the watch channel for broadcasting config updates.
    tx: watch::Sender<PunchConfig>,
    /// Receiver half of the watch channel — cloned for each subscriber.
    rx: watch::Receiver<PunchConfig>,
}

impl ConfigWatcher {
    /// Create a new ConfigWatcher ready to observe the corner team's playbook.
    pub fn new(config_path: PathBuf, initial_config: PunchConfig) -> Self {
        let (tx, rx) = watch::channel(initial_config.clone());
        Self {
            config_path,
            current: Arc::new(RwLock::new(initial_config)),
            tx,
            rx,
        }
    }

    /// Subscribe to config changes — get a ringside seat for every strategy adjustment.
    pub fn subscribe(&self) -> watch::Receiver<PunchConfig> {
        self.rx.clone()
    }

    /// Get a snapshot of the current config — check what game plan the fighter is using right now.
    pub fn current(&self) -> PunchConfig {
        self.rx.borrow().clone()
    }

    /// Start watching the config file for changes — the corner team takes their position.
    ///
    /// Spawns a background task that monitors the config file using filesystem events
    /// and applies validated changes automatically.
    pub async fn start_watching(&self) -> PunchResult<()> {
        let config_path = self.config_path.clone();
        let current = Arc::clone(&self.current);
        let tx = self.tx.clone();

        // Resolve the parent directory and file name for the watcher.
        let watch_path = config_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        let target_file = config_path
            .file_name()
            .map(|f| f.to_os_string())
            .ok_or_else(|| PunchError::Config("config path has no file name".to_string()))?;

        let (notify_tx, mut notify_rx) = tokio::sync::mpsc::channel::<Event>(16);

        // Create the filesystem watcher on a blocking thread since notify uses sync callbacks.
        let _watcher: RecommendedWatcher = {
            let notify_tx = notify_tx.clone();
            let mut watcher =
                notify::recommended_watcher(move |res: Result<Event, notify::Error>| match res {
                    Ok(event) => {
                        if let Err(e) = notify_tx.blocking_send(event) {
                            error!(error = %e, "failed to forward file event");
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "filesystem watcher error");
                    }
                })
                .map_err(|e| PunchError::Config(format!("failed to create file watcher: {}", e)))?;

            watcher
                .watch(&watch_path, RecursiveMode::NonRecursive)
                .map_err(|e| {
                    PunchError::Config(format!("failed to watch config directory: {}", e))
                })?;

            watcher
        };

        let config_path_for_task = config_path.clone();
        let target_file_for_task = target_file.clone();

        // Spawn the background task — the corner team is now watching the fight.
        tokio::spawn(async move {
            // Keep the watcher alive for the lifetime of this task.
            let _watcher = _watcher;

            info!(path = %config_path_for_task.display(), "corner team watching config file");

            while let Some(event) = notify_rx.recv().await {
                // Only react to modify/create events on our target file.
                let dominated = matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_));
                if !dominated {
                    continue;
                }

                let affects_target = event.paths.iter().any(|p| {
                    p.file_name()
                        .map(|f| f == target_file_for_task)
                        .unwrap_or(false)
                });
                if !affects_target {
                    continue;
                }

                debug!("config file change detected, reloading");

                // Read and parse the new config.
                let content = match tokio::fs::read_to_string(&config_path_for_task).await {
                    Ok(c) => c,
                    Err(e) => {
                        error!(error = %e, "failed to read config file during reload");
                        continue;
                    }
                };

                let new_config: PunchConfig = match toml::from_str(&content) {
                    Ok(c) => c,
                    Err(e) => {
                        error!(error = %e, "failed to parse config file during reload");
                        continue;
                    }
                };

                // Validate the new config.
                let errors: Vec<_> = validate_config(&new_config)
                    .into_iter()
                    .filter(|v| matches!(v.severity, ValidationSeverity::Error))
                    .collect();

                if !errors.is_empty() {
                    for err in &errors {
                        error!(field = %err.field, message = %err.message, "config validation failed");
                    }
                    continue;
                }

                // Apply the change.
                let old_config = {
                    let mut guard = current.write().await;
                    let old = guard.clone();
                    *guard = new_config.clone();
                    old
                };

                let changes = diff_configs(&old_config, &new_config);
                if changes.is_empty() {
                    debug!("config file changed but no effective differences detected");
                    continue;
                }

                for change in &changes {
                    info!(change = ?change, "corner team adjustment applied");
                }

                // Broadcast the new config to all subscribers.
                if tx.send(new_config).is_err() {
                    warn!("no config subscribers remaining — corner team shouting into the void");
                    break;
                }
            }

            info!("config watcher task ended");
        });

        Ok(())
    }

    /// Validate and apply a new config programmatically — a direct corner team call.
    ///
    /// Returns the set of changes if the config is valid, or a validation error
    /// if the new config fails checks.
    pub fn apply_change(
        &self,
        new_config: PunchConfig,
    ) -> Result<ConfigChangeSet, ConfigValidationError> {
        let validation_errors: Vec<_> = validate_config(&new_config)
            .into_iter()
            .filter(|v| matches!(v.severity, ValidationSeverity::Error))
            .collect();

        if let Some(err) = validation_errors.into_iter().next() {
            return Err(err);
        }

        let old_config = self.rx.borrow().clone();
        let changes = diff_configs(&old_config, &new_config);

        // Update current config behind the lock (blocking context is fine for apply_change).
        {
            let current = Arc::clone(&self.current);
            let new_config_clone = new_config.clone();
            // Use try_write to avoid blocking in sync context. If contended, fall back to
            // a blocking write via std::thread::spawn, but for apply_change this is acceptable.
            let rt = tokio::runtime::Handle::try_current();
            match rt {
                Ok(handle) => {
                    let current = current.clone();
                    let cfg = new_config_clone.clone();
                    handle.spawn(async move {
                        let mut guard = current.write().await;
                        *guard = cfg;
                    });
                }
                Err(_) => {
                    // If no runtime, we're in a sync context — just best-effort.
                    // The watch channel is the source of truth anyway.
                }
            }
        }

        // Broadcast through the watch channel — this is the authoritative update.
        let _ = self.tx.send(new_config);

        Ok(ConfigChangeSet {
            changes,
            applied_at: Utc::now(),
        })
    }
}

/// A set of changes applied in a single config reload — the corner team's adjustment notes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChangeSet {
    /// Individual changes detected between old and new configs.
    pub changes: Vec<ConfigChange>,
    /// Timestamp when these adjustments were applied.
    pub applied_at: DateTime<Utc>,
}

/// A single configuration change detected during a reload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConfigChange {
    /// The default model was swapped — switching fighting stance.
    ModelChanged {
        old_model: String,
        new_model: String,
    },
    /// API key was rotated — new credentials for the fight.
    ApiKeyChanged,
    /// Rate limit was adjusted — changing the pace of the bout.
    RateLimitChanged { old: u32, new: u32 },
    /// Listen address was changed — moving to a different ring.
    ListenAddressChanged { old: String, new: String },
    /// A new channel entered the arena.
    ChannelAdded(String),
    /// A channel was pulled from the fight card.
    ChannelRemoved(String),
    /// A new MCP server joined the corner team.
    McpServerAdded(String),
    /// An MCP server was cut from the roster.
    McpServerRemoved(String),
    /// Memory configuration was adjusted — changing the fighter's recall strategy.
    MemoryConfigChanged,
}

/// A validation error found in a config — a foul called by the referee.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigValidationError {
    /// The config field that failed validation.
    pub field: String,
    /// Human-readable description of the issue.
    pub message: String,
    /// How severe this validation failure is.
    pub severity: ValidationSeverity,
}

impl std::fmt::Display for ConfigValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{:?}] {}: {}", self.severity, self.field, self.message)
    }
}

impl std::error::Error for ConfigValidationError {}

/// Severity of a configuration validation issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValidationSeverity {
    /// Something worth noting but not a showstopper — the fighter can continue.
    Warning,
    /// A hard foul — the config cannot be accepted.
    Error,
}

/// Compare two configs and enumerate what changed — scouting the opponent's adjustments.
pub fn diff_configs(old: &PunchConfig, new: &PunchConfig) -> Vec<ConfigChange> {
    let mut changes = Vec::new();

    // Model change
    if old.default_model.model != new.default_model.model {
        changes.push(ConfigChange::ModelChanged {
            old_model: old.default_model.model.clone(),
            new_model: new.default_model.model.clone(),
        });
    }

    // API key change
    if old.api_key != new.api_key {
        changes.push(ConfigChange::ApiKeyChanged);
    }

    // Rate limit change
    if old.rate_limit_rpm != new.rate_limit_rpm {
        changes.push(ConfigChange::RateLimitChanged {
            old: old.rate_limit_rpm,
            new: new.rate_limit_rpm,
        });
    }

    // Listen address change
    if old.api_listen != new.api_listen {
        changes.push(ConfigChange::ListenAddressChanged {
            old: old.api_listen.clone(),
            new: new.api_listen.clone(),
        });
    }

    // Channel diffs
    let old_channels: HashSet<&String> = old.channels.keys().collect();
    let new_channels: HashSet<&String> = new.channels.keys().collect();

    for added in new_channels.difference(&old_channels) {
        changes.push(ConfigChange::ChannelAdded((*added).clone()));
    }
    for removed in old_channels.difference(&new_channels) {
        changes.push(ConfigChange::ChannelRemoved((*removed).clone()));
    }

    // MCP server diffs
    let old_servers: HashSet<&String> = old.mcp_servers.keys().collect();
    let new_servers: HashSet<&String> = new.mcp_servers.keys().collect();

    for added in new_servers.difference(&old_servers) {
        changes.push(ConfigChange::McpServerAdded((*added).clone()));
    }
    for removed in old_servers.difference(&new_servers) {
        changes.push(ConfigChange::McpServerRemoved((*removed).clone()));
    }

    // Memory config change — compare serialized forms to catch any field differences.
    let old_mem = serde_json::to_string(&old.memory).unwrap_or_default();
    let new_mem = serde_json::to_string(&new.memory).unwrap_or_default();
    if old_mem != new_mem {
        changes.push(ConfigChange::MemoryConfigChanged);
    }

    changes
}

/// Validate a config for correctness — the referee's pre-fight inspection.
///
/// Returns a list of validation issues. Errors must be fixed before the config
/// can be accepted; warnings are advisory.
pub fn validate_config(config: &PunchConfig) -> Vec<ConfigValidationError> {
    let mut errors = Vec::new();

    // Check api_listen is a valid socket address format.
    if config.api_listen.parse::<std::net::SocketAddr>().is_err() {
        errors.push(ConfigValidationError {
            field: "api_listen".to_string(),
            message: format!(
                "'{}' is not a valid socket address (expected host:port)",
                config.api_listen
            ),
            severity: ValidationSeverity::Error,
        });
    }

    // Check default_model has a non-empty model name.
    if config.default_model.model.trim().is_empty() {
        errors.push(ConfigValidationError {
            field: "default_model.model".to_string(),
            message: "model name cannot be empty — the fighter needs a stance".to_string(),
            severity: ValidationSeverity::Error,
        });
    }

    // Check memory db_path is non-empty.
    if config.memory.db_path.trim().is_empty() {
        errors.push(ConfigValidationError {
            field: "memory.db_path".to_string(),
            message: "database path cannot be empty — the fighter needs memory".to_string(),
            severity: ValidationSeverity::Error,
        });
    }

    // Check rate_limit_rpm is > 0.
    if config.rate_limit_rpm == 0 {
        errors.push(ConfigValidationError {
            field: "rate_limit_rpm".to_string(),
            message: "rate limit must be greater than zero — even a slugger needs some pace"
                .to_string(),
            severity: ValidationSeverity::Error,
        });
    }

    // Warn if api_key is empty (dev mode).
    if config.api_key.is_empty() {
        errors.push(ConfigValidationError {
            field: "api_key".to_string(),
            message: "API key is empty — running in dev mode with no authentication".to_string(),
            severity: ValidationSeverity::Warning,
        });
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MemoryConfig, ModelConfig, Provider};
    use std::collections::HashMap;

    /// Build a valid test config — a well-prepared fighter entering the ring.
    fn make_test_config() -> PunchConfig {
        PunchConfig {
            api_listen: "127.0.0.1:6660".to_string(),
            api_key: "test-key-123".to_string(),
            rate_limit_rpm: 60,
            default_model: ModelConfig {
                provider: Provider::Anthropic,
                model: "claude-sonnet-4-20250514".to_string(),
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                base_url: None,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
            memory: MemoryConfig {
                db_path: "/tmp/punch-test.db".to_string(),
                knowledge_graph_enabled: true,
                max_entries: Some(10000),
            },
            tunnel: None,
            channels: HashMap::new(),
            mcp_servers: HashMap::new(),
            model_routing: Default::default(),
            budget: Default::default(),
        }
    }

    #[test]
    fn diff_detects_model_change() {
        let old = make_test_config();
        let mut new = old.clone();
        new.default_model.model = "claude-opus-4-20250514".to_string();

        let changes = diff_configs(&old, &new);
        assert!(changes.contains(&ConfigChange::ModelChanged {
            old_model: "claude-sonnet-4-20250514".to_string(),
            new_model: "claude-opus-4-20250514".to_string(),
        }));
    }

    #[test]
    fn diff_detects_rate_limit_change() {
        let old = make_test_config();
        let mut new = old.clone();
        new.rate_limit_rpm = 120;

        let changes = diff_configs(&old, &new);
        assert!(changes.contains(&ConfigChange::RateLimitChanged { old: 60, new: 120 }));
    }

    #[test]
    fn diff_detects_channel_added() {
        let old = make_test_config();
        let mut new = old.clone();
        new.channels.insert(
            "slack".to_string(),
            crate::config::ChannelConfig {
                channel_type: "slack".to_string(),
                token_env: Some("SLACK_TOKEN".to_string()),
                webhook_secret_env: None,
                allowed_user_ids: vec![],
                rate_limit_per_user: 20,
                settings: HashMap::new(),
            },
        );

        let changes = diff_configs(&old, &new);
        assert!(changes.contains(&ConfigChange::ChannelAdded("slack".to_string())));
    }

    #[test]
    fn diff_detects_channel_removed() {
        let mut old = make_test_config();
        old.channels.insert(
            "discord".to_string(),
            crate::config::ChannelConfig {
                channel_type: "discord".to_string(),
                token_env: Some("DISCORD_TOKEN".to_string()),
                webhook_secret_env: None,
                allowed_user_ids: vec![],
                rate_limit_per_user: 20,
                settings: HashMap::new(),
            },
        );
        let new = make_test_config();

        let changes = diff_configs(&old, &new);
        assert!(changes.contains(&ConfigChange::ChannelRemoved("discord".to_string())));
    }

    #[test]
    fn diff_returns_empty_for_identical_configs() {
        let config = make_test_config();
        let changes = diff_configs(&config, &config);
        assert!(
            changes.is_empty(),
            "identical configs should produce no changes"
        );
    }

    #[test]
    fn validate_passes_valid_config() {
        let config = make_test_config();
        let errors: Vec<_> = validate_config(&config)
            .into_iter()
            .filter(|e| matches!(e.severity, ValidationSeverity::Error))
            .collect();
        assert!(errors.is_empty(), "valid config should produce no errors");
    }

    #[test]
    fn validate_catches_empty_model_name() {
        let mut config = make_test_config();
        config.default_model.model = "".to_string();

        let errors = validate_config(&config);
        assert!(
            errors.iter().any(|e| e.field == "default_model.model"
                && matches!(e.severity, ValidationSeverity::Error))
        );
    }

    #[test]
    fn validate_catches_empty_db_path() {
        let mut config = make_test_config();
        config.memory.db_path = "".to_string();

        let errors = validate_config(&config);
        assert!(errors.iter().any(
            |e| e.field == "memory.db_path" && matches!(e.severity, ValidationSeverity::Error)
        ));
    }

    #[test]
    fn validate_warns_on_empty_api_key() {
        let mut config = make_test_config();
        config.api_key = "".to_string();

        let errors = validate_config(&config);
        assert!(
            errors
                .iter()
                .any(|e| e.field == "api_key" && matches!(e.severity, ValidationSeverity::Warning))
        );
    }

    #[test]
    fn config_watcher_can_be_created() {
        let config = make_test_config();
        let watcher = ConfigWatcher::new(PathBuf::from("/tmp/punch.toml"), config.clone());
        assert_eq!(watcher.current().api_listen, config.api_listen);
    }

    #[tokio::test]
    async fn apply_change_returns_change_set() {
        let config = make_test_config();
        let watcher = ConfigWatcher::new(PathBuf::from("/tmp/punch.toml"), config);

        let mut new_config = make_test_config();
        new_config.rate_limit_rpm = 120;

        let result = watcher.apply_change(new_config);
        assert!(result.is_ok());
        let change_set = result.expect("should succeed");
        assert!(
            change_set
                .changes
                .contains(&ConfigChange::RateLimitChanged { old: 60, new: 120 })
        );
    }

    #[tokio::test]
    async fn apply_change_rejects_invalid_config() {
        let config = make_test_config();
        let watcher = ConfigWatcher::new(PathBuf::from("/tmp/punch.toml"), config);

        let mut bad_config = make_test_config();
        bad_config.default_model.model = "".to_string();

        let result = watcher.apply_change(bad_config);
        assert!(result.is_err());
    }

    #[test]
    fn current_config_accessible_after_creation() {
        let config = make_test_config();
        let watcher = ConfigWatcher::new(PathBuf::from("/tmp/punch.toml"), config.clone());

        let current = watcher.current();
        assert_eq!(current.api_listen, "127.0.0.1:6660");
        assert_eq!(current.rate_limit_rpm, 60);
        assert_eq!(current.default_model.model, "claude-sonnet-4-20250514");
        assert_eq!(current.memory.db_path, "/tmp/punch-test.db");
    }

    #[test]
    fn diff_detects_mcp_server_added() {
        let old = make_test_config();
        let mut new = old.clone();
        new.mcp_servers.insert(
            "filesystem".to_string(),
            crate::config::McpServerConfig {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "@mcp/filesystem".to_string()],
                env: HashMap::new(),
            },
        );

        let changes = diff_configs(&old, &new);
        assert!(changes.contains(&ConfigChange::McpServerAdded("filesystem".to_string())));
    }

    #[test]
    fn diff_detects_mcp_server_removed() {
        let mut old = make_test_config();
        old.mcp_servers.insert(
            "memory".to_string(),
            crate::config::McpServerConfig {
                command: "mcp-memory".to_string(),
                args: vec![],
                env: HashMap::new(),
            },
        );
        let new = make_test_config();

        let changes = diff_configs(&old, &new);
        assert!(changes.contains(&ConfigChange::McpServerRemoved("memory".to_string())));
    }

    #[test]
    fn diff_detects_memory_config_changed() {
        let old = make_test_config();
        let mut new = old.clone();
        new.memory.knowledge_graph_enabled = false;

        let changes = diff_configs(&old, &new);
        assert!(changes.contains(&ConfigChange::MemoryConfigChanged));
    }

    #[test]
    fn diff_detects_api_key_changed() {
        let old = make_test_config();
        let mut new = old.clone();
        new.api_key = "new-secret-key".to_string();

        let changes = diff_configs(&old, &new);
        assert!(changes.contains(&ConfigChange::ApiKeyChanged));
    }

    #[test]
    fn diff_detects_listen_address_changed() {
        let old = make_test_config();
        let mut new = old.clone();
        new.api_listen = "0.0.0.0:8080".to_string();

        let changes = diff_configs(&old, &new);
        assert!(changes.contains(&ConfigChange::ListenAddressChanged {
            old: "127.0.0.1:6660".to_string(),
            new: "0.0.0.0:8080".to_string(),
        }));
    }

    #[test]
    fn validate_catches_invalid_socket_addr() {
        let mut config = make_test_config();
        config.api_listen = "not-a-valid-address".to_string();

        let errors = validate_config(&config);
        assert!(errors.iter().any(|e| e.field == "api_listen"
            && matches!(e.severity, ValidationSeverity::Error)));
    }

    #[test]
    fn validate_catches_zero_rate_limit() {
        let mut config = make_test_config();
        config.rate_limit_rpm = 0;

        let errors = validate_config(&config);
        assert!(errors.iter().any(
            |e| e.field == "rate_limit_rpm" && matches!(e.severity, ValidationSeverity::Error)
        ));
    }

    #[tokio::test]
    async fn subscriber_receives_initial_config() {
        let config = make_test_config();
        let watcher = ConfigWatcher::new(PathBuf::from("/tmp/punch.toml"), config.clone());

        let rx = watcher.subscribe();
        let received = rx.borrow().clone();
        assert_eq!(received.api_listen, config.api_listen);
    }
}
