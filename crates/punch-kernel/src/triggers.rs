//! Event-driven trigger engine for the Punch Agent Combat System.
//!
//! The [`TriggerEngine`] manages triggers that automatically fire actions
//! when conditions are met. Supports scheduled triggers (cron-like),
//! keyword matching in messages, event pattern matching, and webhook triggers.
//!
//! Inspired by OpenFang's trigger system, adapted for Punch's combat metaphor.

use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};
use uuid::Uuid;

use punch_types::{FighterId, GorillaId, PunchEvent};

// ---------------------------------------------------------------------------
// TriggerId
// ---------------------------------------------------------------------------

/// Unique identifier for a trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TriggerId(pub Uuid);

impl TriggerId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TriggerId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TriggerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Trigger types
// ---------------------------------------------------------------------------

/// What kind of condition activates a trigger.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum TriggerCondition {
    /// Fire on a cron-like schedule (interval in seconds).
    Schedule {
        /// Interval in seconds between fires.
        interval_secs: u64,
    },
    /// Fire when a message contains one of the specified keywords (case-insensitive).
    Keyword {
        /// Keywords to match against (any match triggers).
        keywords: Vec<String>,
    },
    /// Fire when a specific [`PunchEvent`] variant occurs.
    Event {
        /// The event kind to match (e.g. "fighter_spawned", "gorilla_unleashed").
        event_kind: String,
    },
    /// Fire when an HTTP webhook is received.
    Webhook {
        /// Optional secret for webhook validation.
        secret: Option<String>,
    },
}

/// What action to perform when a trigger fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum TriggerAction {
    /// Spawn a fighter from a template name.
    SpawnFighter { template_name: String },
    /// Send a message to a specific fighter.
    SendMessage {
        fighter_id: FighterId,
        message: String,
    },
    /// Execute a workflow by ID.
    ExecuteWorkflow { workflow_id: String, input: String },
    /// Trigger a single gorilla tick.
    RunGorilla { gorilla_id: GorillaId },
    /// Log a message (useful for testing and debugging).
    Log { message: String },
}

/// A registered trigger definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger {
    /// Unique trigger ID.
    pub id: TriggerId,
    /// Human-readable name.
    pub name: String,
    /// The condition that activates this trigger.
    pub condition: TriggerCondition,
    /// The action to perform when triggered.
    pub action: TriggerAction,
    /// Whether this trigger is currently active.
    pub enabled: bool,
    /// When this trigger was created.
    pub created_at: DateTime<Utc>,
    /// How many times this trigger has fired.
    pub fire_count: u64,
    /// Maximum number of times this trigger can fire (0 = unlimited).
    pub max_fires: u64,
}

/// Summary information about a trigger (for listing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerSummary {
    /// Human-readable name.
    pub name: String,
    /// Description of the condition.
    pub condition_type: String,
    /// Whether active.
    pub enabled: bool,
    /// How many times fired.
    pub fire_count: u64,
    /// When created.
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// TriggerEngine
// ---------------------------------------------------------------------------

/// The trigger engine manages event-to-action routing.
pub struct TriggerEngine {
    /// All registered triggers.
    triggers: DashMap<TriggerId, Trigger>,
}

impl TriggerEngine {
    /// Create a new trigger engine.
    pub fn new() -> Self {
        Self {
            triggers: DashMap::new(),
        }
    }

    /// Register a new trigger and return its ID.
    pub fn register_trigger(&self, trigger: Trigger) -> TriggerId {
        let id = trigger.id;
        info!(trigger_id = %id, name = %trigger.name, "trigger registered");
        self.triggers.insert(id, trigger);
        id
    }

    /// Remove a trigger by ID.
    pub fn remove_trigger(&self, id: &TriggerId) {
        if let Some((_, trigger)) = self.triggers.remove(id) {
            info!(trigger_id = %id, name = %trigger.name, "trigger removed");
        }
    }

    /// List all triggers with summary information.
    pub fn list_triggers(&self) -> Vec<(TriggerId, TriggerSummary)> {
        self.triggers
            .iter()
            .map(|entry| {
                let t = entry.value();
                let condition_type = match &t.condition {
                    TriggerCondition::Schedule { interval_secs } => {
                        format!("schedule({}s)", interval_secs)
                    }
                    TriggerCondition::Keyword { keywords } => {
                        format!("keyword({})", keywords.join(", "))
                    }
                    TriggerCondition::Event { event_kind } => {
                        format!("event({})", event_kind)
                    }
                    TriggerCondition::Webhook { .. } => "webhook".to_string(),
                };
                (
                    *entry.key(),
                    TriggerSummary {
                        name: t.name.clone(),
                        condition_type,
                        enabled: t.enabled,
                        fire_count: t.fire_count,
                        created_at: t.created_at,
                    },
                )
            })
            .collect()
    }

    /// Check if a message matches any keyword triggers.
    ///
    /// Returns the IDs of all matching triggers and increments their fire counts.
    pub async fn check_keyword(&self, message: &str) -> Vec<TriggerId> {
        let lower_message = message.to_lowercase();
        let mut matched = Vec::new();

        for mut entry in self.triggers.iter_mut() {
            let trigger = entry.value_mut();
            if !trigger.enabled {
                continue;
            }
            if trigger.max_fires > 0 && trigger.fire_count >= trigger.max_fires {
                trigger.enabled = false;
                continue;
            }

            if let TriggerCondition::Keyword { keywords } = &trigger.condition {
                let is_match = keywords
                    .iter()
                    .any(|kw| lower_message.contains(&kw.to_lowercase()));
                if is_match {
                    trigger.fire_count += 1;
                    matched.push(trigger.id);
                    debug!(
                        trigger_id = %trigger.id,
                        name = %trigger.name,
                        fire_count = trigger.fire_count,
                        "keyword trigger fired"
                    );
                }
            }
        }

        matched
    }

    /// Check if a [`PunchEvent`] matches any event triggers.
    ///
    /// Returns the IDs of all matching triggers and increments their fire counts.
    pub async fn check_event(&self, event: &PunchEvent) -> Vec<TriggerId> {
        let event_kind = event_kind_string(event);
        let mut matched = Vec::new();

        for mut entry in self.triggers.iter_mut() {
            let trigger = entry.value_mut();
            if !trigger.enabled {
                continue;
            }
            if trigger.max_fires > 0 && trigger.fire_count >= trigger.max_fires {
                trigger.enabled = false;
                continue;
            }

            if let TriggerCondition::Event {
                event_kind: pattern,
            } = &trigger.condition
                && (pattern == "*" || pattern == &event_kind)
            {
                trigger.fire_count += 1;
                matched.push(trigger.id);
                debug!(
                    trigger_id = %trigger.id,
                    name = %trigger.name,
                    event_kind = %event_kind,
                    "event trigger fired"
                );
            }
        }

        matched
    }

    /// Get all schedule-type triggers with their intervals.
    pub fn get_schedule_triggers(&self) -> Vec<(TriggerId, Duration)> {
        self.triggers
            .iter()
            .filter_map(|entry| {
                let t = entry.value();
                if !t.enabled {
                    return None;
                }
                if let TriggerCondition::Schedule { interval_secs } = &t.condition {
                    Some((*entry.key(), Duration::from_secs(*interval_secs)))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get a trigger by ID.
    pub fn get_trigger(&self, id: &TriggerId) -> Option<Trigger> {
        self.triggers.get(id).map(|t| t.clone())
    }

    /// Check if a webhook trigger exists and return its action.
    pub fn check_webhook(&self, id: &TriggerId) -> Option<TriggerAction> {
        let mut entry = self.triggers.get_mut(id)?;
        let trigger = entry.value_mut();

        if !trigger.enabled {
            return None;
        }
        if trigger.max_fires > 0 && trigger.fire_count >= trigger.max_fires {
            trigger.enabled = false;
            return None;
        }

        if matches!(trigger.condition, TriggerCondition::Webhook { .. }) {
            trigger.fire_count += 1;
            debug!(
                trigger_id = %trigger.id,
                name = %trigger.name,
                "webhook trigger fired"
            );
            Some(trigger.action.clone())
        } else {
            None
        }
    }
}

impl Default for TriggerEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map a [`PunchEvent`] to a string kind for matching.
fn event_kind_string(event: &PunchEvent) -> String {
    match event {
        PunchEvent::FighterSpawned { .. } => "fighter_spawned".to_string(),
        PunchEvent::FighterMessage { .. } => "fighter_message".to_string(),
        PunchEvent::GorillaUnleashed { .. } => "gorilla_unleashed".to_string(),
        PunchEvent::GorillaPaused { .. } => "gorilla_paused".to_string(),
        PunchEvent::ToolExecuted { .. } => "tool_executed".to_string(),
        PunchEvent::BoutStarted { .. } => "bout_started".to_string(),
        PunchEvent::BoutEnded { .. } => "bout_ended".to_string(),
        PunchEvent::ComboTriggered { .. } => "combo_triggered".to_string(),
        PunchEvent::TroopFormed { .. } => "troop_formed".to_string(),
        PunchEvent::TroopDisbanded { .. } => "troop_disbanded".to_string(),
        PunchEvent::Error { .. } => "error".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::FighterId;

    fn make_keyword_trigger(keywords: Vec<&str>) -> Trigger {
        Trigger {
            id: TriggerId::new(),
            name: "test-keyword".to_string(),
            condition: TriggerCondition::Keyword {
                keywords: keywords.into_iter().map(String::from).collect(),
            },
            action: TriggerAction::Log {
                message: "keyword matched".to_string(),
            },
            enabled: true,
            created_at: Utc::now(),
            fire_count: 0,
            max_fires: 0,
        }
    }

    fn make_event_trigger(event_kind: &str) -> Trigger {
        Trigger {
            id: TriggerId::new(),
            name: "test-event".to_string(),
            condition: TriggerCondition::Event {
                event_kind: event_kind.to_string(),
            },
            action: TriggerAction::Log {
                message: "event matched".to_string(),
            },
            enabled: true,
            created_at: Utc::now(),
            fire_count: 0,
            max_fires: 0,
        }
    }

    fn make_schedule_trigger(interval_secs: u64) -> Trigger {
        Trigger {
            id: TriggerId::new(),
            name: "test-schedule".to_string(),
            condition: TriggerCondition::Schedule { interval_secs },
            action: TriggerAction::Log {
                message: "schedule fired".to_string(),
            },
            enabled: true,
            created_at: Utc::now(),
            fire_count: 0,
            max_fires: 0,
        }
    }

    #[tokio::test]
    async fn test_keyword_trigger_matching() {
        let engine = TriggerEngine::new();
        let trigger = make_keyword_trigger(vec!["deploy", "release"]);
        let id = engine.register_trigger(trigger);

        // Should match.
        let matches = engine.check_keyword("please deploy the app").await;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], id);

        // Should match (case-insensitive).
        let matches = engine.check_keyword("DEPLOY now!").await;
        assert_eq!(matches.len(), 1);

        // Should not match.
        let matches = engine.check_keyword("hello world").await;
        assert!(matches.is_empty());
    }

    #[tokio::test]
    async fn test_keyword_trigger_multiple_keywords() {
        let engine = TriggerEngine::new();
        let trigger = make_keyword_trigger(vec!["help", "assist"]);
        engine.register_trigger(trigger);

        let matches = engine.check_keyword("I need help").await;
        assert_eq!(matches.len(), 1);

        let matches = engine.check_keyword("please assist me").await;
        assert_eq!(matches.len(), 1);
    }

    #[tokio::test]
    async fn test_event_trigger_firing() {
        let engine = TriggerEngine::new();
        let trigger = make_event_trigger("fighter_spawned");
        let id = engine.register_trigger(trigger);

        let event = PunchEvent::FighterSpawned {
            fighter_id: FighterId::new(),
            name: "test".to_string(),
        };

        let matches = engine.check_event(&event).await;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], id);

        // Different event type should not match.
        let event2 = PunchEvent::Error {
            source: "test".to_string(),
            message: "oops".to_string(),
        };
        let matches2 = engine.check_event(&event2).await;
        assert!(matches2.is_empty());
    }

    #[tokio::test]
    async fn test_event_trigger_wildcard() {
        let engine = TriggerEngine::new();
        let trigger = make_event_trigger("*");
        engine.register_trigger(trigger);

        let event = PunchEvent::Error {
            source: "test".to_string(),
            message: "anything".to_string(),
        };
        let matches = engine.check_event(&event).await;
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_schedule_trigger_listing() {
        let engine = TriggerEngine::new();
        let t1 = make_schedule_trigger(60);
        let t2 = make_schedule_trigger(300);
        engine.register_trigger(t1);
        engine.register_trigger(t2);

        // Also add a non-schedule trigger to verify it's excluded.
        let t3 = make_keyword_trigger(vec!["hello"]);
        engine.register_trigger(t3);

        let schedules = engine.get_schedule_triggers();
        assert_eq!(schedules.len(), 2);
    }

    #[test]
    fn test_trigger_registration_and_removal() {
        let engine = TriggerEngine::new();
        let trigger = make_keyword_trigger(vec!["test"]);
        let id = engine.register_trigger(trigger);

        assert!(engine.get_trigger(&id).is_some());
        assert_eq!(engine.list_triggers().len(), 1);

        engine.remove_trigger(&id);
        assert!(engine.get_trigger(&id).is_none());
        assert_eq!(engine.list_triggers().len(), 0);
    }

    #[tokio::test]
    async fn test_trigger_max_fires() {
        let engine = TriggerEngine::new();
        let mut trigger = make_keyword_trigger(vec!["fire"]);
        trigger.max_fires = 2;
        engine.register_trigger(trigger);

        // First two should match.
        assert_eq!(engine.check_keyword("fire").await.len(), 1);
        assert_eq!(engine.check_keyword("fire").await.len(), 1);
        // Third should not.
        assert_eq!(engine.check_keyword("fire").await.len(), 0);
    }

    #[tokio::test]
    async fn test_disabled_trigger_does_not_fire() {
        let engine = TriggerEngine::new();
        let mut trigger = make_keyword_trigger(vec!["test"]);
        trigger.enabled = false;
        engine.register_trigger(trigger);

        let matches = engine.check_keyword("test message").await;
        assert!(matches.is_empty());
    }

    #[test]
    fn test_webhook_trigger() {
        let engine = TriggerEngine::new();
        let trigger = Trigger {
            id: TriggerId::new(),
            name: "webhook-test".to_string(),
            condition: TriggerCondition::Webhook { secret: None },
            action: TriggerAction::Log {
                message: "webhook received".to_string(),
            },
            enabled: true,
            created_at: Utc::now(),
            fire_count: 0,
            max_fires: 0,
        };
        let id = engine.register_trigger(trigger);

        let action = engine.check_webhook(&id);
        assert!(action.is_some());

        // Non-existent ID should return None.
        let fake_id = TriggerId::new();
        assert!(engine.check_webhook(&fake_id).is_none());
    }

    #[test]
    fn trigger_engine_default() {
        let engine = TriggerEngine::default();
        assert!(engine.list_triggers().is_empty());
    }

    #[test]
    fn trigger_id_display() {
        let id = TriggerId::new();
        let s = format!("{}", id);
        assert!(!s.is_empty());
    }

    #[test]
    fn trigger_id_default() {
        let id = TriggerId::default();
        assert!(!id.0.is_nil());
    }

    #[test]
    fn get_trigger_returns_correct_data() {
        let engine = TriggerEngine::new();
        let trigger = make_keyword_trigger(vec!["hello"]);
        let id = engine.register_trigger(trigger);

        let retrieved = engine.get_trigger(&id).unwrap();
        assert_eq!(retrieved.name, "test-keyword");
        assert!(retrieved.enabled);
        assert_eq!(retrieved.fire_count, 0);
    }

    #[test]
    fn get_trigger_nonexistent_returns_none() {
        let engine = TriggerEngine::new();
        let id = TriggerId::new();
        assert!(engine.get_trigger(&id).is_none());
    }

    #[test]
    fn remove_nonexistent_trigger_does_not_panic() {
        let engine = TriggerEngine::new();
        let id = TriggerId::new();
        engine.remove_trigger(&id); // Should not panic.
    }

    #[tokio::test]
    async fn keyword_trigger_fire_count_increments() {
        let engine = TriggerEngine::new();
        let trigger = make_keyword_trigger(vec!["count"]);
        let id = engine.register_trigger(trigger);

        engine.check_keyword("count me").await;
        engine.check_keyword("count again").await;
        engine.check_keyword("count three").await;

        let t = engine.get_trigger(&id).unwrap();
        assert_eq!(t.fire_count, 3);
    }

    #[tokio::test]
    async fn event_trigger_fire_count_increments() {
        let engine = TriggerEngine::new();
        let trigger = make_event_trigger("error");
        let id = engine.register_trigger(trigger);

        let event = PunchEvent::Error {
            source: "test".to_string(),
            message: "oops".to_string(),
        };
        engine.check_event(&event).await;
        engine.check_event(&event).await;

        let t = engine.get_trigger(&id).unwrap();
        assert_eq!(t.fire_count, 2);
    }

    #[test]
    fn webhook_trigger_fire_count_increments() {
        let engine = TriggerEngine::new();
        let trigger = Trigger {
            id: TriggerId::new(),
            name: "webhook-count".to_string(),
            condition: TriggerCondition::Webhook { secret: Some("secret".to_string()) },
            action: TriggerAction::Log { message: "fired".to_string() },
            enabled: true,
            created_at: Utc::now(),
            fire_count: 0,
            max_fires: 0,
        };
        let id = engine.register_trigger(trigger);

        engine.check_webhook(&id);
        engine.check_webhook(&id);

        let t = engine.get_trigger(&id).unwrap();
        assert_eq!(t.fire_count, 2);
    }

    #[test]
    fn webhook_trigger_disabled_returns_none() {
        let engine = TriggerEngine::new();
        let trigger = Trigger {
            id: TriggerId::new(),
            name: "disabled-webhook".to_string(),
            condition: TriggerCondition::Webhook { secret: None },
            action: TriggerAction::Log { message: "nope".to_string() },
            enabled: false,
            created_at: Utc::now(),
            fire_count: 0,
            max_fires: 0,
        };
        let id = trigger.id;
        engine.register_trigger(trigger);

        assert!(engine.check_webhook(&id).is_none());
    }

    #[test]
    fn webhook_trigger_max_fires_reached() {
        let engine = TriggerEngine::new();
        let trigger = Trigger {
            id: TriggerId::new(),
            name: "limited-webhook".to_string(),
            condition: TriggerCondition::Webhook { secret: None },
            action: TriggerAction::Log { message: "limited".to_string() },
            enabled: true,
            created_at: Utc::now(),
            fire_count: 0,
            max_fires: 1,
        };
        let id = engine.register_trigger(trigger);

        assert!(engine.check_webhook(&id).is_some());
        // Second should fail (max_fires=1 reached).
        assert!(engine.check_webhook(&id).is_none());
    }

    #[test]
    fn check_webhook_on_non_webhook_trigger_returns_none() {
        let engine = TriggerEngine::new();
        let trigger = make_keyword_trigger(vec!["test"]);
        let id = engine.register_trigger(trigger);

        assert!(engine.check_webhook(&id).is_none());
    }

    #[test]
    fn disabled_schedule_trigger_excluded() {
        let engine = TriggerEngine::new();
        let mut trigger = make_schedule_trigger(60);
        trigger.enabled = false;
        engine.register_trigger(trigger);

        let schedules = engine.get_schedule_triggers();
        assert!(schedules.is_empty());
    }

    #[test]
    fn list_triggers_returns_summaries() {
        let engine = TriggerEngine::new();
        let t1 = make_keyword_trigger(vec!["a", "b"]);
        let t2 = make_event_trigger("fighter_spawned");
        let t3 = make_schedule_trigger(120);
        let t4 = Trigger {
            id: TriggerId::new(),
            name: "webhook".to_string(),
            condition: TriggerCondition::Webhook { secret: None },
            action: TriggerAction::Log { message: "wh".to_string() },
            enabled: true,
            created_at: Utc::now(),
            fire_count: 0,
            max_fires: 0,
        };

        engine.register_trigger(t1);
        engine.register_trigger(t2);
        engine.register_trigger(t3);
        engine.register_trigger(t4);

        let summaries = engine.list_triggers();
        assert_eq!(summaries.len(), 4);

        // Check condition_type descriptions.
        let types: Vec<String> = summaries.iter().map(|(_, s)| s.condition_type.clone()).collect();
        assert!(types.iter().any(|t| t.contains("keyword")));
        assert!(types.iter().any(|t| t.contains("event")));
        assert!(types.iter().any(|t| t.contains("schedule")));
        assert!(types.iter().any(|t| t == "webhook"));
    }

    #[tokio::test]
    async fn multiple_keyword_triggers_fire_independently() {
        let engine = TriggerEngine::new();
        let t1 = make_keyword_trigger(vec!["alpha"]);
        let t2 = make_keyword_trigger(vec!["beta"]);
        let id1 = engine.register_trigger(t1);
        let id2 = engine.register_trigger(t2);

        // Only alpha matches.
        let matches = engine.check_keyword("alpha is here").await;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], id1);

        // Only beta matches.
        let matches = engine.check_keyword("beta is here").await;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], id2);

        // Both match.
        let matches = engine.check_keyword("alpha and beta together").await;
        assert_eq!(matches.len(), 2);
    }
}
