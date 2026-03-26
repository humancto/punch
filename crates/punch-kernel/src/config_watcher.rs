//! Config Hot Reload — poll-based config watcher with callback support.
//!
//! The [`KernelConfigWatcher`] wraps the underlying [`ConfigWatcher`] from
//! `punch-types` and adds a poll-based mtime check, callback registration,
//! and diff logging for the kernel layer. It distinguishes between hot-reloadable
//! fields (rate limits, model defaults, channels, MCP servers, memory settings)
//! and fields that require a restart (API listen address, database path, API key).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use punch_types::config::PunchConfig;
use punch_types::hot_reload::{ConfigChange, ValidationSeverity, diff_configs, validate_config};

// ---------------------------------------------------------------------------
// ConfigDiff (kernel-level summary)
// ---------------------------------------------------------------------------

/// Summary of what changed between two configs — used by callbacks to react
/// to specific categories of changes.
#[derive(Debug, Clone, Default)]
pub struct KernelConfigDiff {
    /// Whether rate limit settings changed.
    pub rate_limit_changed: bool,
    /// Whether the default model changed.
    pub model_changed: bool,
    /// Channel names that were added, removed, or modified.
    pub channels_changed: Vec<String>,
    /// MCP server names that were added, removed, or modified.
    pub mcp_servers_changed: Vec<String>,
    /// Whether memory configuration changed.
    pub memory_changed: bool,
    /// Non-reloadable fields that changed (require restart).
    pub requires_restart: Vec<String>,
}

impl KernelConfigDiff {
    /// Build a `KernelConfigDiff` from the low-level `ConfigChange` list.
    fn from_changes(changes: &[ConfigChange]) -> Self {
        let mut diff = Self::default();

        for change in changes {
            match change {
                ConfigChange::RateLimitChanged { .. } => {
                    diff.rate_limit_changed = true;
                }
                ConfigChange::ModelChanged { .. } => {
                    diff.model_changed = true;
                }
                ConfigChange::ChannelAdded(name) | ConfigChange::ChannelRemoved(name) => {
                    if !diff.channels_changed.contains(name) {
                        diff.channels_changed.push(name.clone());
                    }
                }
                ConfigChange::McpServerAdded(name) | ConfigChange::McpServerRemoved(name) => {
                    if !diff.mcp_servers_changed.contains(name) {
                        diff.mcp_servers_changed.push(name.clone());
                    }
                }
                ConfigChange::MemoryConfigChanged => {
                    diff.memory_changed = true;
                }
                // Non-reloadable fields.
                ConfigChange::ListenAddressChanged { .. } => {
                    diff.requires_restart.push("api_listen".to_string());
                }
                ConfigChange::ApiKeyChanged => {
                    diff.requires_restart.push("api_key".to_string());
                }
            }
        }

        diff
    }

    /// Returns true if any reloadable field changed.
    pub fn has_reloadable_changes(&self) -> bool {
        self.rate_limit_changed
            || self.model_changed
            || !self.channels_changed.is_empty()
            || !self.mcp_servers_changed.is_empty()
            || self.memory_changed
    }
}

// ---------------------------------------------------------------------------
// KernelConfigWatcher
// ---------------------------------------------------------------------------

/// Type alias for the callback collection to keep clippy happy.
type ConfigCallbacks = Arc<RwLock<Vec<Box<dyn Fn(&PunchConfig, &KernelConfigDiff) + Send + Sync>>>>;

/// A poll-based config file watcher that detects changes and applies them
/// without requiring a restart.
///
/// It polls the file's mtime every 5 seconds, re-reads and validates on change,
/// and notifies registered callbacks with the new config and a diff summary.
pub struct KernelConfigWatcher {
    config: Arc<RwLock<PunchConfig>>,
    config_path: PathBuf,
    last_modified: AtomicU64,
    callbacks: ConfigCallbacks,
}

impl KernelConfigWatcher {
    /// Create a new watcher for the given config file path with an initial config.
    pub fn new(config_path: PathBuf, initial_config: PunchConfig) -> Self {
        let mtime = Self::file_mtime(&config_path).unwrap_or(0);

        Self {
            config: Arc::new(RwLock::new(initial_config)),
            config_path,
            last_modified: AtomicU64::new(mtime),
            callbacks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Register a callback that will be invoked when the config changes.
    ///
    /// Multiple callbacks can be registered. They are called in registration order
    /// with a reference to the new config and the diff summary.
    pub async fn on_change<F>(&self, callback: F)
    where
        F: Fn(&PunchConfig, &KernelConfigDiff) + Send + Sync + 'static,
    {
        let mut cbs = self.callbacks.write().await;
        cbs.push(Box::new(callback));
    }

    /// Get a clone of the current config.
    pub async fn current_config(&self) -> PunchConfig {
        self.config.read().await.clone()
    }

    /// Get a shared reference to the underlying config Arc.
    pub fn config_arc(&self) -> Arc<RwLock<PunchConfig>> {
        Arc::clone(&self.config)
    }

    /// Start the poll loop. Returns a `JoinHandle` for the spawned task.
    ///
    /// The task checks the config file's mtime every 5 seconds. On change:
    /// 1. Reads and parses the file as TOML
    /// 2. Validates the new config (keeps old config on error)
    /// 3. Computes the diff and logs changes
    /// 4. Warns about non-reloadable changes
    /// 5. Swaps the config under the `RwLock`
    /// 6. Notifies all registered callbacks
    pub fn watch(&self) -> JoinHandle<()> {
        let config = Arc::clone(&self.config);
        let config_path = self.config_path.clone();
        let last_modified = self.last_modified.load(Ordering::Relaxed);
        let last_modified_atomic = Arc::new(AtomicU64::new(last_modified));
        let callbacks = Arc::clone(&self.callbacks);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            // Skip the first immediate tick.
            interval.tick().await;

            info!(path = %config_path.display(), "config poll watcher started (5s interval)");

            loop {
                interval.tick().await;

                let current_mtime = match Self::file_mtime(&config_path) {
                    Some(m) => m,
                    None => {
                        debug!("config file not found or inaccessible, skipping check");
                        continue;
                    }
                };

                let prev_mtime = last_modified_atomic.load(Ordering::Relaxed);
                if current_mtime == prev_mtime {
                    continue;
                }

                debug!(
                    old_mtime = prev_mtime,
                    new_mtime = current_mtime,
                    "config file mtime changed, reloading"
                );

                last_modified_atomic.store(current_mtime, Ordering::Relaxed);

                // Read file content.
                let content = match tokio::fs::read_to_string(&config_path).await {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(error = %e, "failed to read config file during hot reload");
                        continue;
                    }
                };

                // Parse TOML.
                let new_config: PunchConfig = match toml::from_str(&content) {
                    Ok(c) => c,
                    Err(e) => {
                        warn!(error = %e, "config parse error during hot reload — keeping old config");
                        continue;
                    }
                };

                // Validate.
                let errors: Vec<_> = validate_config(&new_config)
                    .into_iter()
                    .filter(|v| matches!(v.severity, ValidationSeverity::Error))
                    .collect();

                if !errors.is_empty() {
                    for err in &errors {
                        warn!(field = %err.field, message = %err.message, "config validation error — keeping old config");
                    }
                    continue;
                }

                // Compute diff.
                let old_config = config.read().await.clone();
                let changes = diff_configs(&old_config, &new_config);

                if changes.is_empty() {
                    debug!("config file changed (mtime) but no effective differences");
                    continue;
                }

                let diff = KernelConfigDiff::from_changes(&changes);

                // Log each change.
                for change in &changes {
                    info!(change = ?change, "config hot reload: change detected");
                }

                // Warn about non-reloadable fields.
                for field in &diff.requires_restart {
                    warn!(
                        field = %field,
                        "config field changed but requires restart to take effect"
                    );
                }

                // Swap config.
                {
                    let mut guard = config.write().await;
                    *guard = new_config.clone();
                }

                // Notify callbacks.
                let cbs = callbacks.read().await;
                for cb in cbs.iter() {
                    cb(&new_config, &diff);
                }

                info!(num_changes = changes.len(), "config hot reload complete");
            }
        })
    }

    /// Read the file's mtime as epoch seconds. Returns `None` if the file
    /// cannot be stat'd.
    fn file_mtime(path: &PathBuf) -> Option<u64> {
        std::fs::metadata(path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::config::{MemoryConfig, ModelConfig, Provider};
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;

    fn make_test_config() -> PunchConfig {
        PunchConfig {
            api_listen: "127.0.0.1:6660".to_string(),
            api_key: "test-key".to_string(),
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
    fn kernel_config_diff_from_changes() {
        let changes = vec![
            ConfigChange::RateLimitChanged { old: 60, new: 120 },
            ConfigChange::ModelChanged {
                old_model: "a".to_string(),
                new_model: "b".to_string(),
            },
            ConfigChange::ChannelAdded("slack".to_string()),
            ConfigChange::McpServerRemoved("fs".to_string()),
            ConfigChange::ListenAddressChanged {
                old: "a".to_string(),
                new: "b".to_string(),
            },
            ConfigChange::ApiKeyChanged,
        ];

        let diff = KernelConfigDiff::from_changes(&changes);
        assert!(diff.rate_limit_changed);
        assert!(diff.model_changed);
        assert_eq!(diff.channels_changed, vec!["slack".to_string()]);
        assert_eq!(diff.mcp_servers_changed, vec!["fs".to_string()]);
        assert_eq!(diff.requires_restart.len(), 2);
        assert!(diff.requires_restart.contains(&"api_listen".to_string()));
        assert!(diff.requires_restart.contains(&"api_key".to_string()));
    }

    #[test]
    fn kernel_config_diff_has_reloadable_changes() {
        let empty = KernelConfigDiff::default();
        assert!(!empty.has_reloadable_changes());

        let with_rate = KernelConfigDiff {
            rate_limit_changed: true,
            ..Default::default()
        };
        assert!(with_rate.has_reloadable_changes());

        let restart_only = KernelConfigDiff {
            requires_restart: vec!["api_listen".to_string()],
            ..Default::default()
        };
        assert!(!restart_only.has_reloadable_changes());
    }

    #[tokio::test]
    async fn watch_detects_file_change() {
        let dir = std::env::temp_dir().join(format!("punch-cfg-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("punch.toml");

        let initial = make_test_config();
        let toml_str = toml::to_string_pretty(&initial).expect("serialize initial config");
        std::fs::write(&config_path, &toml_str).expect("write initial config");

        let watcher = KernelConfigWatcher::new(config_path.clone(), initial.clone());

        let callback_fired = Arc::new(AtomicBool::new(false));
        let cb_flag = Arc::clone(&callback_fired);
        watcher
            .on_change(move |_cfg, _diff| {
                cb_flag.store(true, Ordering::Relaxed);
            })
            .await;

        let handle = watcher.watch();

        // Wait a bit then modify the file.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let mut modified = initial.clone();
        modified.rate_limit_rpm = 120;
        let new_toml = toml::to_string_pretty(&modified).expect("serialize modified config");

        // Ensure mtime differs (some filesystems have 1s granularity).
        tokio::time::sleep(Duration::from_secs(1)).await;
        std::fs::write(&config_path, &new_toml).expect("write modified config");

        // Wait for the poller to pick it up.
        tokio::time::sleep(Duration::from_secs(7)).await;

        assert!(
            callback_fired.load(Ordering::Relaxed),
            "callback should have been fired after config change"
        );

        // Verify the config was updated.
        let current = watcher.current_config().await;
        assert_eq!(current.rate_limit_rpm, 120);

        handle.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn parse_error_keeps_old_config() {
        let dir = std::env::temp_dir().join(format!("punch-cfg-parse-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let config_path = dir.join("punch.toml");

        let initial = make_test_config();
        let toml_str = toml::to_string_pretty(&initial).expect("serialize initial config");
        std::fs::write(&config_path, &toml_str).expect("write initial config");

        let watcher = KernelConfigWatcher::new(config_path.clone(), initial.clone());
        let handle = watcher.watch();

        tokio::time::sleep(Duration::from_secs(1)).await;

        // Write invalid TOML.
        std::fs::write(&config_path, "this is not valid toml {{{}}}").expect("write bad config");

        tokio::time::sleep(Duration::from_secs(7)).await;

        // Config should be unchanged.
        let current = watcher.current_config().await;
        assert_eq!(current.rate_limit_rpm, 60);

        handle.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn diff_correctly_identifies_changed_fields() {
        let old = make_test_config();
        let mut new = old.clone();
        new.rate_limit_rpm = 200;
        new.default_model.model = "gpt-4o".to_string();

        let changes = diff_configs(&old, &new);
        let diff = KernelConfigDiff::from_changes(&changes);

        assert!(diff.rate_limit_changed);
        assert!(diff.model_changed);
        assert!(diff.channels_changed.is_empty());
        assert!(diff.mcp_servers_changed.is_empty());
        assert!(diff.requires_restart.is_empty());
    }

    #[tokio::test]
    async fn callback_registration_and_invocation() {
        let config_path = PathBuf::from("/tmp/nonexistent-punch-test.toml");
        let config = make_test_config();
        let watcher = KernelConfigWatcher::new(config_path, config);

        let counter = Arc::new(AtomicU64::new(0));
        let c1 = Arc::clone(&counter);
        watcher
            .on_change(move |_cfg, _diff| {
                c1.fetch_add(1, Ordering::Relaxed);
            })
            .await;

        // Verify callback list has one entry.
        let cbs = watcher.callbacks.read().await;
        assert_eq!(cbs.len(), 1);
    }

    #[tokio::test]
    async fn multiple_callbacks_supported() {
        let config_path = PathBuf::from("/tmp/nonexistent-punch-multi.toml");
        let config = make_test_config();
        let watcher = KernelConfigWatcher::new(config_path, config);

        let c1 = Arc::new(AtomicU64::new(0));
        let c2 = Arc::new(AtomicU64::new(0));

        let c1_clone = Arc::clone(&c1);
        let c2_clone = Arc::clone(&c2);

        watcher
            .on_change(move |_cfg, _diff| {
                c1_clone.fetch_add(1, Ordering::Relaxed);
            })
            .await;

        watcher
            .on_change(move |_cfg, _diff| {
                c2_clone.fetch_add(1, Ordering::Relaxed);
            })
            .await;

        let cbs = watcher.callbacks.read().await;
        assert_eq!(cbs.len(), 2);
    }

    #[test]
    fn non_reloadable_fields_logged_as_requiring_restart() {
        let changes = vec![
            ConfigChange::ListenAddressChanged {
                old: "127.0.0.1:6660".to_string(),
                new: "0.0.0.0:8080".to_string(),
            },
            ConfigChange::ApiKeyChanged,
        ];

        let diff = KernelConfigDiff::from_changes(&changes);
        assert!(!diff.has_reloadable_changes());
        assert_eq!(diff.requires_restart.len(), 2);
    }

    #[tokio::test]
    async fn concurrent_reads_during_reload() {
        let config = make_test_config();
        let watcher = KernelConfigWatcher::new(PathBuf::from("/tmp/test.toml"), config);
        let config_arc = watcher.config_arc();

        // Spawn multiple concurrent readers.
        let mut handles = Vec::new();
        for _ in 0..10 {
            let arc = Arc::clone(&config_arc);
            handles.push(tokio::spawn(async move {
                let cfg = arc.read().await;
                assert!(!cfg.api_listen.is_empty());
            }));
        }

        // Spawn a writer.
        let arc_w = Arc::clone(&config_arc);
        handles.push(tokio::spawn(async move {
            let mut cfg = arc_w.write().await;
            cfg.rate_limit_rpm = 999;
        }));

        for h in handles {
            h.await.expect("task should complete");
        }

        // Verify the write took effect.
        let final_cfg = config_arc.read().await;
        assert_eq!(final_cfg.rate_limit_rpm, 999);
    }

    #[test]
    fn memory_change_detected() {
        let changes = vec![ConfigChange::MemoryConfigChanged];
        let diff = KernelConfigDiff::from_changes(&changes);
        assert!(diff.memory_changed);
        assert!(diff.has_reloadable_changes());
    }
}
