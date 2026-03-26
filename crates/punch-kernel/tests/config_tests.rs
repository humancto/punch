//! Integration tests for config hot reload, config diffing, and the
//! KernelConfigWatcher callback system.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use punch_kernel::{KernelConfigDiff, KernelConfigWatcher};
use punch_types::config::{MemoryConfig, PunchConfig};
use punch_types::hot_reload::{ConfigChange, diff_configs, validate_config};
use punch_types::{ModelConfig, Provider};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_config() -> PunchConfig {
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

// ===========================================================================
// KernelConfigDiff tests
// ===========================================================================

/// Empty changes produce an empty diff.
#[test]
fn test_diff_from_empty_changes() {
    let diff = KernelConfigDiff::default();
    assert!(!diff.rate_limit_changed);
    assert!(!diff.model_changed);
    assert!(diff.channels_changed.is_empty());
    assert!(diff.mcp_servers_changed.is_empty());
    assert!(!diff.memory_changed);
    assert!(diff.requires_restart.is_empty());
    assert!(!diff.has_reloadable_changes());
}

/// Rate limit change is detected as reloadable.
#[test]
fn test_diff_rate_limit_change_is_reloadable() {
    let old = make_config();
    let mut new = old.clone();
    new.rate_limit_rpm = 120;

    let changes = diff_configs(&old, &new);
    assert!(!changes.is_empty());

    // Find RateLimitChanged.
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, ConfigChange::RateLimitChanged { .. }))
    );
}

/// Model change is detected.
#[test]
fn test_diff_model_change_detected() {
    let old = make_config();
    let mut new = old.clone();
    new.default_model.model = "gpt-4o".to_string();

    let changes = diff_configs(&old, &new);
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, ConfigChange::ModelChanged { .. }))
    );
}

/// API listen address change requires restart.
#[test]
fn test_diff_listen_address_requires_restart() {
    let old = make_config();
    let mut new = old.clone();
    new.api_listen = "0.0.0.0:8080".to_string();

    let changes = diff_configs(&old, &new);
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, ConfigChange::ListenAddressChanged { .. }))
    );
}

/// API key change requires restart.
#[test]
fn test_diff_api_key_requires_restart() {
    let old = make_config();
    let mut new = old.clone();
    new.api_key = "new-key".to_string();

    let changes = diff_configs(&old, &new);
    assert!(
        changes
            .iter()
            .any(|c| matches!(c, ConfigChange::ApiKeyChanged))
    );
}

/// Identical configs produce no changes.
#[test]
fn test_diff_identical_configs_no_changes() {
    let config = make_config();
    let changes = diff_configs(&config, &config);
    assert!(changes.is_empty());
}

// ===========================================================================
// Config validation tests
// ===========================================================================

/// Valid config passes validation.
#[test]
fn test_validate_valid_config() {
    let config = make_config();
    let errors = validate_config(&config);
    let hard_errors: Vec<_> = errors
        .iter()
        .filter(|v| {
            matches!(
                v.severity,
                punch_types::hot_reload::ValidationSeverity::Error
            )
        })
        .collect();
    assert!(
        hard_errors.is_empty(),
        "valid config should have no errors: {:?}",
        hard_errors
    );
}

// ===========================================================================
// KernelConfigWatcher tests
// ===========================================================================

/// current_config returns the initial config.
#[tokio::test]
async fn test_watcher_current_config_returns_initial() {
    let config = make_config();
    let watcher = KernelConfigWatcher::new(
        PathBuf::from("/tmp/nonexistent-config.toml"),
        config.clone(),
    );

    let current = watcher.current_config().await;
    assert_eq!(current.rate_limit_rpm, 60);
    assert_eq!(current.api_listen, "127.0.0.1:6660");
}

/// Registering a callback increases the callback count.
#[tokio::test]
async fn test_watcher_register_callback() {
    let watcher = KernelConfigWatcher::new(PathBuf::from("/tmp/test-watcher.toml"), make_config());

    let counter = Arc::new(AtomicU64::new(0));
    let c = Arc::clone(&counter);
    watcher
        .on_change(move |_cfg, _diff| {
            c.fetch_add(1, Ordering::Relaxed);
        })
        .await;

    // The callback is registered but not yet invoked.
    assert_eq!(counter.load(Ordering::Relaxed), 0);
}

/// Multiple callbacks can be registered.
#[tokio::test]
async fn test_watcher_multiple_callbacks() {
    let watcher = KernelConfigWatcher::new(PathBuf::from("/tmp/multi-cb-test.toml"), make_config());

    let c1 = Arc::new(AtomicU64::new(0));
    let c2 = Arc::new(AtomicU64::new(0));

    let c1c = Arc::clone(&c1);
    watcher
        .on_change(move |_cfg, _diff| {
            c1c.fetch_add(1, Ordering::Relaxed);
        })
        .await;

    let c2c = Arc::clone(&c2);
    watcher
        .on_change(move |_cfg, _diff| {
            c2c.fetch_add(1, Ordering::Relaxed);
        })
        .await;

    // Both registered, neither invoked yet.
    assert_eq!(c1.load(Ordering::Relaxed), 0);
    assert_eq!(c2.load(Ordering::Relaxed), 0);
}

/// config_arc returns a shared reference to the config.
#[tokio::test]
async fn test_watcher_config_arc_readable() {
    let config = make_config();
    let watcher = KernelConfigWatcher::new(PathBuf::from("/tmp/arc-test.toml"), config);
    let arc = watcher.config_arc();

    let guard = arc.read().await;
    assert_eq!(guard.rate_limit_rpm, 60);
}

/// Concurrent readers on config_arc do not deadlock.
#[tokio::test]
async fn test_watcher_concurrent_reads() {
    let watcher = KernelConfigWatcher::new(PathBuf::from("/tmp/conc-test.toml"), make_config());
    let arc = watcher.config_arc();

    let mut handles = Vec::new();
    for _ in 0..10 {
        let a = Arc::clone(&arc);
        handles.push(tokio::spawn(async move {
            let cfg = a.read().await;
            assert!(!cfg.api_listen.is_empty());
        }));
    }

    for h in handles {
        h.await.expect("concurrent read should succeed");
    }
}

/// Watch detects file changes and fires callbacks.
#[tokio::test]
async fn test_watcher_detects_file_change() {
    let dir = std::env::temp_dir().join(format!("punch-cfg-integ-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let config_path = dir.join("punch.toml");

    let initial = make_config();
    let toml_str = toml::to_string_pretty(&initial).expect("serialize");
    std::fs::write(&config_path, &toml_str).expect("write initial");

    let watcher = KernelConfigWatcher::new(config_path.clone(), initial.clone());

    let callback_fired = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&callback_fired);
    watcher
        .on_change(move |_cfg, _diff| {
            flag.store(true, Ordering::Relaxed);
        })
        .await;

    let handle = watcher.watch();

    // Wait then modify.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut modified = initial.clone();
    modified.rate_limit_rpm = 200;
    let new_toml = toml::to_string_pretty(&modified).expect("serialize modified");
    std::fs::write(&config_path, &new_toml).expect("write modified");

    // Wait for poller.
    tokio::time::sleep(Duration::from_secs(7)).await;

    assert!(
        callback_fired.load(Ordering::Relaxed),
        "callback should fire after config change"
    );

    let current = watcher.current_config().await;
    assert_eq!(current.rate_limit_rpm, 200);

    handle.abort();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Invalid TOML preserves the old config.
#[tokio::test]
async fn test_watcher_invalid_toml_keeps_old() {
    let dir = std::env::temp_dir().join(format!("punch-cfg-bad-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let config_path = dir.join("punch.toml");

    let initial = make_config();
    let toml_str = toml::to_string_pretty(&initial).expect("serialize");
    std::fs::write(&config_path, &toml_str).expect("write initial");

    let watcher = KernelConfigWatcher::new(config_path.clone(), initial);
    let handle = watcher.watch();

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Write invalid TOML.
    std::fs::write(&config_path, "{{{{ broken }}}").expect("write bad config");

    tokio::time::sleep(Duration::from_secs(7)).await;

    let current = watcher.current_config().await;
    assert_eq!(
        current.rate_limit_rpm, 60,
        "should keep old config on parse error"
    );

    handle.abort();
    let _ = std::fs::remove_dir_all(&dir);
}
