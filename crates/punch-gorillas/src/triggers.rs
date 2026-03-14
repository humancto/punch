//! Gorilla Trigger System — event-driven gorilla activation.
//!
//! Provides multiple trigger types for activating gorillas: cron-based,
//! webhook, file watch, message-based, manual, and chain triggers.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tracing::{debug, info};

use punch_types::{GorillaId, PunchError, PunchResult};

// ---------------------------------------------------------------------------
// TriggerType
// ---------------------------------------------------------------------------

/// The type of event that activates a gorilla.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerType {
    /// Time-based scheduling (cron expression or interval).
    Cron {
        /// The schedule expression (cron or interval).
        schedule: String,
    },
    /// HTTP webhook endpoint triggers the gorilla.
    Webhook {
        /// Path suffix for the webhook endpoint.
        path: String,
        /// Optional secret for webhook validation.
        secret: Option<String>,
    },
    /// Filesystem change triggers the gorilla.
    FileWatch {
        /// Paths to watch.
        paths: Vec<PathBuf>,
        /// File patterns to match (globs).
        patterns: Vec<String>,
    },
    /// Incoming message on a channel triggers the gorilla.
    Message {
        /// Channel name or pattern to watch.
        channel: String,
        /// Keywords that trigger activation (empty = any message).
        keywords: Vec<String>,
    },
    /// Explicit API call triggers the gorilla.
    Manual,
    /// Another gorilla completing triggers this gorilla.
    Chain {
        /// The gorilla whose completion triggers this one.
        source_gorilla: GorillaId,
        /// Only trigger on success (true) or any completion (false).
        on_success_only: bool,
    },
}

// ---------------------------------------------------------------------------
// GorillaTriggerId
// ---------------------------------------------------------------------------

/// Unique identifier for a gorilla trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GorillaTriggerId(pub uuid::Uuid);

impl GorillaTriggerId {
    /// Create a new random trigger ID.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }
}

impl Default for GorillaTriggerId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for GorillaTriggerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// GorillaTriger
// ---------------------------------------------------------------------------

/// A registered trigger that can activate a gorilla.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GorillaTrigger {
    /// Unique trigger ID.
    pub id: GorillaTriggerId,
    /// Which gorilla this trigger activates.
    pub gorilla_id: GorillaId,
    /// Human-readable trigger name.
    pub name: String,
    /// The trigger type and its configuration.
    pub trigger_type: TriggerType,
    /// Whether this trigger is currently active.
    pub enabled: bool,
    /// When this trigger was created.
    pub created_at: DateTime<Utc>,
    /// How many times this trigger has fired.
    pub fire_count: u64,
    /// Maximum fire count (0 = unlimited).
    pub max_fires: u64,
    /// Last time this trigger fired.
    pub last_fired: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Trigger trait
// ---------------------------------------------------------------------------

/// Trait for objects that can evaluate whether a trigger condition is met.
pub trait TriggerEvaluator: Send + Sync {
    /// Check if the trigger should fire given the current context.
    fn should_fire(&self, trigger: &GorillaTrigger) -> bool;

    /// Get a human-readable description of the trigger type.
    fn description(&self) -> String;
}

/// Cron trigger evaluator.
pub struct CronEvaluator {
    /// The current time for evaluation.
    pub now: DateTime<Utc>,
}

impl TriggerEvaluator for CronEvaluator {
    fn should_fire(&self, trigger: &GorillaTrigger) -> bool {
        if let TriggerType::Cron { .. } = &trigger.trigger_type {
            // Cron triggers are handled by the scheduler, so this always returns true
            // when called from the scheduler's tick.
            true
        } else {
            false
        }
    }

    fn description(&self) -> String {
        "cron/schedule trigger".to_string()
    }
}

/// Manual trigger evaluator (always fires when called).
pub struct ManualEvaluator;

impl TriggerEvaluator for ManualEvaluator {
    fn should_fire(&self, trigger: &GorillaTrigger) -> bool {
        matches!(trigger.trigger_type, TriggerType::Manual)
    }

    fn description(&self) -> String {
        "manual trigger".to_string()
    }
}

/// Message trigger evaluator.
pub struct MessageEvaluator {
    /// The incoming message content.
    pub message: String,
    /// The channel the message came from.
    pub channel: String,
}

impl TriggerEvaluator for MessageEvaluator {
    fn should_fire(&self, trigger: &GorillaTrigger) -> bool {
        if let TriggerType::Message { channel, keywords } = &trigger.trigger_type {
            // Check channel match.
            if channel != "*" && channel != &self.channel {
                return false;
            }
            // If no keywords, any message triggers.
            if keywords.is_empty() {
                return true;
            }
            // Check keyword match.
            let lower = self.message.to_lowercase();
            keywords.iter().any(|kw| lower.contains(&kw.to_lowercase()))
        } else {
            false
        }
    }

    fn description(&self) -> String {
        format!("message trigger on channel '{}'", self.channel)
    }
}

/// Chain trigger evaluator — fires when a source gorilla completes.
pub struct ChainEvaluator {
    /// The gorilla that completed.
    pub completed_gorilla: GorillaId,
    /// Whether the completion was successful.
    pub success: bool,
}

impl TriggerEvaluator for ChainEvaluator {
    fn should_fire(&self, trigger: &GorillaTrigger) -> bool {
        if let TriggerType::Chain {
            source_gorilla,
            on_success_only,
        } = &trigger.trigger_type
        {
            if *source_gorilla != self.completed_gorilla {
                return false;
            }
            if *on_success_only && !self.success {
                return false;
            }
            true
        } else {
            false
        }
    }

    fn description(&self) -> String {
        format!("chain trigger from gorilla {}", self.completed_gorilla)
    }
}

/// Webhook trigger evaluator.
pub struct WebhookEvaluator {
    /// The path the webhook was received on.
    pub path: String,
}

impl TriggerEvaluator for WebhookEvaluator {
    fn should_fire(&self, trigger: &GorillaTrigger) -> bool {
        if let TriggerType::Webhook { path, .. } = &trigger.trigger_type {
            path == &self.path || path == "*"
        } else {
            false
        }
    }

    fn description(&self) -> String {
        format!("webhook trigger on path '{}'", self.path)
    }
}

/// File watch trigger evaluator.
pub struct FileWatchEvaluator {
    /// The path that changed.
    pub changed_path: PathBuf,
}

impl TriggerEvaluator for FileWatchEvaluator {
    fn should_fire(&self, trigger: &GorillaTrigger) -> bool {
        if let TriggerType::FileWatch { paths, patterns } = &trigger.trigger_type {
            // Check if changed path is under any watched path.
            let under_watched = paths.iter().any(|p| self.changed_path.starts_with(p));
            if !under_watched && !paths.is_empty() {
                return false;
            }
            // If no patterns, any change triggers.
            if patterns.is_empty() {
                return true;
            }
            // Check pattern match (simple suffix matching).
            let path_str = self.changed_path.to_string_lossy();
            patterns.iter().any(|pattern| {
                if let Some(ext) = pattern.strip_prefix("*.") {
                    path_str.ends_with(&format!(".{}", ext))
                } else {
                    path_str.contains(pattern)
                }
            })
        } else {
            false
        }
    }

    fn description(&self) -> String {
        format!("file watch trigger for '{}'", self.changed_path.display())
    }
}

// ---------------------------------------------------------------------------
// GorillaTriggerEngine
// ---------------------------------------------------------------------------

/// Engine for managing gorilla triggers.
pub struct GorillaTriggerEngine {
    /// All registered triggers.
    triggers: DashMap<GorillaTriggerId, GorillaTrigger>,
    /// Index: gorilla_id → list of trigger IDs.
    gorilla_triggers: DashMap<GorillaId, Vec<GorillaTriggerId>>,
    /// Notification for trigger fires.
    notify: Arc<Notify>,
}

impl GorillaTriggerEngine {
    /// Create a new trigger engine.
    pub fn new() -> Self {
        Self {
            triggers: DashMap::new(),
            gorilla_triggers: DashMap::new(),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Register a trigger.
    pub fn register(&self, trigger: GorillaTrigger) -> GorillaTriggerId {
        let id = trigger.id;
        let gorilla_id = trigger.gorilla_id;
        info!(
            trigger_id = %id,
            gorilla_id = %gorilla_id,
            name = %trigger.name,
            "gorilla trigger registered"
        );
        self.triggers.insert(id, trigger);

        // Update the gorilla → triggers index.
        self.gorilla_triggers
            .entry(gorilla_id)
            .or_default()
            .push(id);

        id
    }

    /// Remove a trigger.
    pub fn remove(&self, trigger_id: &GorillaTriggerId) {
        if let Some((_, trigger)) = self.triggers.remove(trigger_id) {
            // Remove from gorilla index.
            if let Some(mut ids) = self.gorilla_triggers.get_mut(&trigger.gorilla_id) {
                ids.retain(|id| id != trigger_id);
            }
            info!(trigger_id = %trigger_id, "gorilla trigger removed");
        }
    }

    /// Evaluate all triggers against an evaluator and return the gorilla IDs that should fire.
    pub fn evaluate(&self, evaluator: &dyn TriggerEvaluator) -> Vec<GorillaId> {
        let mut fired = Vec::new();

        for mut entry in self.triggers.iter_mut() {
            let trigger = entry.value_mut();
            if !trigger.enabled {
                continue;
            }
            if trigger.max_fires > 0 && trigger.fire_count >= trigger.max_fires {
                trigger.enabled = false;
                continue;
            }

            if evaluator.should_fire(trigger) {
                trigger.fire_count += 1;
                trigger.last_fired = Some(Utc::now());
                fired.push(trigger.gorilla_id);
                debug!(
                    trigger_id = %trigger.id,
                    gorilla_id = %trigger.gorilla_id,
                    fire_count = trigger.fire_count,
                    "gorilla trigger fired"
                );
            }
        }

        if !fired.is_empty() {
            self.notify.notify_one();
        }

        fired
    }

    /// Get all triggers for a gorilla.
    pub fn get_triggers_for_gorilla(&self, gorilla_id: &GorillaId) -> Vec<GorillaTrigger> {
        self.gorilla_triggers
            .get(gorilla_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.triggers.get(id).map(|t| t.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get a trigger by ID.
    pub fn get(&self, trigger_id: &GorillaTriggerId) -> Option<GorillaTrigger> {
        self.triggers.get(trigger_id).map(|t| t.clone())
    }

    /// List all triggers.
    pub fn list(&self) -> Vec<GorillaTrigger> {
        self.triggers.iter().map(|e| e.value().clone()).collect()
    }

    /// Enable a trigger.
    pub fn enable(&self, trigger_id: &GorillaTriggerId) -> PunchResult<()> {
        let mut entry = self
            .triggers
            .get_mut(trigger_id)
            .ok_or_else(|| PunchError::Gorilla(format!("trigger {} not found", trigger_id)))?;
        entry.enabled = true;
        Ok(())
    }

    /// Disable a trigger.
    pub fn disable(&self, trigger_id: &GorillaTriggerId) -> PunchResult<()> {
        let mut entry = self
            .triggers
            .get_mut(trigger_id)
            .ok_or_else(|| PunchError::Gorilla(format!("trigger {} not found", trigger_id)))?;
        entry.enabled = false;
        Ok(())
    }

    /// Get the notification handle.
    pub fn notifier(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
    }
}

impl Default for GorillaTriggerEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to create a new trigger.
pub fn new_trigger(gorilla_id: GorillaId, name: &str, trigger_type: TriggerType) -> GorillaTrigger {
    GorillaTrigger {
        id: GorillaTriggerId::new(),
        gorilla_id,
        name: name.to_string(),
        trigger_type,
        enabled: true,
        created_at: Utc::now(),
        fire_count: 0,
        max_fires: 0,
        last_fired: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cron_trigger(gid: GorillaId) -> GorillaTrigger {
        new_trigger(
            gid,
            "cron-test",
            TriggerType::Cron {
                schedule: "*/5 * * * *".to_string(),
            },
        )
    }

    fn make_manual_trigger(gid: GorillaId) -> GorillaTrigger {
        new_trigger(gid, "manual-test", TriggerType::Manual)
    }

    fn make_message_trigger(gid: GorillaId, keywords: Vec<&str>) -> GorillaTrigger {
        new_trigger(
            gid,
            "message-test",
            TriggerType::Message {
                channel: "general".to_string(),
                keywords: keywords.into_iter().map(String::from).collect(),
            },
        )
    }

    fn make_chain_trigger(gid: GorillaId, source: GorillaId) -> GorillaTrigger {
        new_trigger(
            gid,
            "chain-test",
            TriggerType::Chain {
                source_gorilla: source,
                on_success_only: true,
            },
        )
    }

    fn make_webhook_trigger(gid: GorillaId) -> GorillaTrigger {
        new_trigger(
            gid,
            "webhook-test",
            TriggerType::Webhook {
                path: "/hooks/test".to_string(),
                secret: None,
            },
        )
    }

    fn make_filewatch_trigger(gid: GorillaId) -> GorillaTrigger {
        new_trigger(
            gid,
            "filewatch-test",
            TriggerType::FileWatch {
                paths: vec![PathBuf::from("/tmp")],
                patterns: vec!["*.log".to_string()],
            },
        )
    }

    #[test]
    fn register_and_list_triggers() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        engine.register(make_cron_trigger(gid));
        engine.register(make_manual_trigger(gid));

        let list = engine.list();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn remove_trigger() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        let trigger = make_cron_trigger(gid);
        let id = engine.register(trigger);
        engine.remove(&id);
        assert!(engine.get(&id).is_none());
    }

    #[test]
    fn manual_trigger_fires() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        engine.register(make_manual_trigger(gid));

        let evaluator = ManualEvaluator;
        let fired = engine.evaluate(&evaluator);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0], gid);
    }

    #[test]
    fn cron_trigger_fires() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        engine.register(make_cron_trigger(gid));

        let evaluator = CronEvaluator { now: Utc::now() };
        let fired = engine.evaluate(&evaluator);
        assert_eq!(fired.len(), 1);
    }

    #[test]
    fn message_trigger_keyword_match() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        engine.register(make_message_trigger(gid, vec!["deploy", "release"]));

        let evaluator = MessageEvaluator {
            message: "please deploy the app".to_string(),
            channel: "general".to_string(),
        };
        let fired = engine.evaluate(&evaluator);
        assert_eq!(fired.len(), 1);
    }

    #[test]
    fn message_trigger_no_match() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        engine.register(make_message_trigger(gid, vec!["deploy"]));

        let evaluator = MessageEvaluator {
            message: "hello world".to_string(),
            channel: "general".to_string(),
        };
        let fired = engine.evaluate(&evaluator);
        assert!(fired.is_empty());
    }

    #[test]
    fn message_trigger_wrong_channel() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        engine.register(make_message_trigger(gid, vec!["deploy"]));

        let evaluator = MessageEvaluator {
            message: "deploy now".to_string(),
            channel: "random".to_string(),
        };
        let fired = engine.evaluate(&evaluator);
        assert!(fired.is_empty());
    }

    #[test]
    fn message_trigger_empty_keywords_matches_all() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        engine.register(make_message_trigger(gid, vec![]));

        let evaluator = MessageEvaluator {
            message: "anything at all".to_string(),
            channel: "general".to_string(),
        };
        let fired = engine.evaluate(&evaluator);
        assert_eq!(fired.len(), 1);
    }

    #[test]
    fn chain_trigger_fires_on_success() {
        let engine = GorillaTriggerEngine::new();
        let source = GorillaId::new();
        let target = GorillaId::new();
        engine.register(make_chain_trigger(target, source));

        let evaluator = ChainEvaluator {
            completed_gorilla: source,
            success: true,
        };
        let fired = engine.evaluate(&evaluator);
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0], target);
    }

    #[test]
    fn chain_trigger_does_not_fire_on_failure() {
        let engine = GorillaTriggerEngine::new();
        let source = GorillaId::new();
        let target = GorillaId::new();
        engine.register(make_chain_trigger(target, source));

        let evaluator = ChainEvaluator {
            completed_gorilla: source,
            success: false,
        };
        let fired = engine.evaluate(&evaluator);
        assert!(fired.is_empty());
    }

    #[test]
    fn chain_trigger_wrong_source() {
        let engine = GorillaTriggerEngine::new();
        let source = GorillaId::new();
        let target = GorillaId::new();
        engine.register(make_chain_trigger(target, source));

        let evaluator = ChainEvaluator {
            completed_gorilla: GorillaId::new(),
            success: true,
        };
        let fired = engine.evaluate(&evaluator);
        assert!(fired.is_empty());
    }

    #[test]
    fn webhook_trigger_fires() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        engine.register(make_webhook_trigger(gid));

        let evaluator = WebhookEvaluator {
            path: "/hooks/test".to_string(),
        };
        let fired = engine.evaluate(&evaluator);
        assert_eq!(fired.len(), 1);
    }

    #[test]
    fn webhook_trigger_wrong_path() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        engine.register(make_webhook_trigger(gid));

        let evaluator = WebhookEvaluator {
            path: "/hooks/other".to_string(),
        };
        let fired = engine.evaluate(&evaluator);
        assert!(fired.is_empty());
    }

    #[test]
    fn filewatch_trigger_fires() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        engine.register(make_filewatch_trigger(gid));

        let evaluator = FileWatchEvaluator {
            changed_path: PathBuf::from("/tmp/app.log"),
        };
        let fired = engine.evaluate(&evaluator);
        assert_eq!(fired.len(), 1);
    }

    #[test]
    fn filewatch_trigger_wrong_extension() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        engine.register(make_filewatch_trigger(gid));

        let evaluator = FileWatchEvaluator {
            changed_path: PathBuf::from("/tmp/app.txt"),
        };
        let fired = engine.evaluate(&evaluator);
        assert!(fired.is_empty());
    }

    #[test]
    fn filewatch_trigger_wrong_directory() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        engine.register(make_filewatch_trigger(gid));

        let evaluator = FileWatchEvaluator {
            changed_path: PathBuf::from("/var/app.log"),
        };
        let fired = engine.evaluate(&evaluator);
        assert!(fired.is_empty());
    }

    #[test]
    fn disabled_trigger_does_not_fire() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        let mut trigger = make_manual_trigger(gid);
        trigger.enabled = false;
        engine.register(trigger);

        let fired = engine.evaluate(&ManualEvaluator);
        assert!(fired.is_empty());
    }

    #[test]
    fn max_fires_respected() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        let mut trigger = make_manual_trigger(gid);
        trigger.max_fires = 2;
        engine.register(trigger);

        assert_eq!(engine.evaluate(&ManualEvaluator).len(), 1);
        assert_eq!(engine.evaluate(&ManualEvaluator).len(), 1);
        assert_eq!(engine.evaluate(&ManualEvaluator).len(), 0);
    }

    #[test]
    fn enable_disable_trigger() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        let trigger = make_manual_trigger(gid);
        let id = engine.register(trigger);

        engine.disable(&id).unwrap();
        assert!(engine.evaluate(&ManualEvaluator).is_empty());

        engine.enable(&id).unwrap();
        assert_eq!(engine.evaluate(&ManualEvaluator).len(), 1);
    }

    #[test]
    fn get_triggers_for_gorilla() {
        let engine = GorillaTriggerEngine::new();
        let gid1 = GorillaId::new();
        let gid2 = GorillaId::new();

        engine.register(make_cron_trigger(gid1));
        engine.register(make_manual_trigger(gid1));
        engine.register(make_cron_trigger(gid2));

        let triggers1 = engine.get_triggers_for_gorilla(&gid1);
        assert_eq!(triggers1.len(), 2);

        let triggers2 = engine.get_triggers_for_gorilla(&gid2);
        assert_eq!(triggers2.len(), 1);
    }

    #[test]
    fn trigger_engine_default() {
        let engine = GorillaTriggerEngine::default();
        assert!(engine.list().is_empty());
    }

    #[test]
    fn trigger_id_display() {
        let id = GorillaTriggerId::new();
        let s = format!("{}", id);
        assert!(!s.is_empty());
    }

    #[test]
    fn trigger_id_default() {
        let id = GorillaTriggerId::default();
        assert!(!id.0.is_nil());
    }

    #[test]
    fn trigger_fire_count_increments() {
        let engine = GorillaTriggerEngine::new();
        let gid = GorillaId::new();
        let trigger = make_manual_trigger(gid);
        let id = engine.register(trigger);

        engine.evaluate(&ManualEvaluator);
        engine.evaluate(&ManualEvaluator);

        let t = engine.get(&id).unwrap();
        assert_eq!(t.fire_count, 2);
        assert!(t.last_fired.is_some());
    }

    #[test]
    fn new_trigger_helper() {
        let gid = GorillaId::new();
        let trigger = new_trigger(gid, "test", TriggerType::Manual);
        assert_eq!(trigger.name, "test");
        assert_eq!(trigger.gorilla_id, gid);
        assert!(trigger.enabled);
        assert_eq!(trigger.fire_count, 0);
    }

    #[test]
    fn trigger_serialization() {
        let gid = GorillaId::new();
        let trigger = new_trigger(gid, "ser-test", TriggerType::Manual);
        let json = serde_json::to_string(&trigger).unwrap();
        let deser: GorillaTrigger = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.name, "ser-test");
    }

    #[test]
    fn enable_nonexistent_trigger() {
        let engine = GorillaTriggerEngine::new();
        let id = GorillaTriggerId::new();
        assert!(engine.enable(&id).is_err());
    }

    #[test]
    fn disable_nonexistent_trigger() {
        let engine = GorillaTriggerEngine::new();
        let id = GorillaTriggerId::new();
        assert!(engine.disable(&id).is_err());
    }
}
