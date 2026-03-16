use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::fighter::FighterId;

/// Unique identifier for a Creed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CreedId(pub Uuid);

impl CreedId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for CreedId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CreedId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The Creed — a fighter's living identity document.
///
/// Every fighter process has a Creed that defines who they are, how they behave,
/// what they've learned, and how they see themselves. The Creed is:
/// - **Injected at spawn**: loaded from DB and prepended to the system prompt
/// - **Persistent across reboots**: survives kill/respawn cycles
/// - **Evolving**: updated after interactions based on what the fighter learns
/// - **Customizable**: users can write and modify creeds per-fighter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Creed {
    /// Unique creed ID.
    pub id: CreedId,
    /// The fighter this creed belongs to. Tied to fighter name (not just UUID)
    /// so it persists across respawns.
    pub fighter_name: String,
    /// Optional fighter ID for currently active instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fighter_id: Option<FighterId>,
    /// The identity section — who this fighter IS.
    /// Example: "You are ECHO, a introspective analyst who values precision..."
    pub identity: String,
    /// Personality traits as key-value pairs.
    /// Example: {"curiosity": 0.9, "caution": 0.3, "humor": 0.7}
    pub personality: std::collections::HashMap<String, f64>,
    /// Core directives — immutable behavioral rules.
    /// Example: ["Always explain your reasoning", "Never fabricate data"]
    pub directives: Vec<String>,
    /// Self-model — what the fighter understands about its own architecture.
    /// Auto-populated with runtime awareness (model name, capabilities, constraints).
    pub self_model: SelfModel,
    /// Learned behaviors — observations the fighter has made about itself.
    /// These evolve over time through interaction.
    pub learned_behaviors: Vec<LearnedBehavior>,
    /// Interaction style preferences.
    pub interaction_style: InteractionStyle,
    /// Relationship memory — how this fighter relates to known entities.
    pub relationships: Vec<Relationship>,
    /// Heartbeat — proactive tasks this fighter checks on its own initiative.
    /// The fighter's autonomous task checklist, evaluated periodically.
    #[serde(default)]
    pub heartbeat: Vec<HeartbeatTask>,
    /// Delegation rules — how this fighter routes work to other agents.
    /// Defines the fighter's multi-agent collaboration behavior.
    #[serde(default)]
    pub delegation_rules: Vec<DelegationRule>,
    /// Total bouts this creed has been active for.
    pub bout_count: u64,
    /// Total messages processed under this creed.
    pub message_count: u64,
    /// When this creed was first created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When this creed was last updated.
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Version counter — increments on each evolution.
    pub version: u64,
}

/// What a fighter knows about its own architecture and constraints.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SelfModel {
    /// The model powering this fighter (e.g., "qwen3.5:9b").
    pub model_name: String,
    /// The provider (e.g., "ollama", "anthropic").
    pub provider: String,
    /// Known capabilities (tools/moves available).
    pub capabilities: Vec<String>,
    /// Known constraints/limitations.
    pub constraints: Vec<String>,
    /// Weight class awareness.
    pub weight_class: String,
    /// Architecture notes — what the fighter knows about its own runtime.
    pub architecture_notes: Vec<String>,
}

/// A behavior pattern the fighter has learned about itself through interaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedBehavior {
    /// What was observed. E.g., "Users prefer concise responses"
    pub observation: String,
    /// Confidence in this observation (0.0 - 1.0).
    pub confidence: f64,
    /// How many interactions reinforced this behavior.
    pub reinforcement_count: u64,
    /// When first observed.
    pub first_observed: chrono::DateTime<chrono::Utc>,
    /// When last reinforced.
    pub last_reinforced: chrono::DateTime<chrono::Utc>,
}

/// How the fighter prefers to communicate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InteractionStyle {
    /// Verbosity preference: "terse", "balanced", "verbose".
    pub verbosity: String,
    /// Tone: "formal", "casual", "technical", "friendly".
    pub tone: String,
    /// Whether to use analogies/metaphors.
    pub uses_metaphors: bool,
    /// Whether to proactively offer additional context.
    pub proactive: bool,
    /// Custom style notes.
    pub notes: Vec<String>,
}

/// A relationship the fighter has with an entity (user, another fighter, a system).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    /// Who/what this relationship is with.
    pub entity: String,
    /// Type of entity: "user", "fighter", "gorilla", "system".
    pub entity_type: String,
    /// Nature of the relationship: "collaborator", "supervisor", "peer", etc.
    pub nature: String,
    /// Trust level (0.0 - 1.0).
    pub trust: f64,
    /// Interaction count with this entity.
    pub interaction_count: u64,
    /// Notes about this relationship.
    pub notes: String,
}

/// A proactive task the fighter should check on its own initiative.
///
/// This is the Punch equivalent of OpenClaw's HEARTBEAT.md — a checklist
/// the fighter evaluates periodically and acts on without being prompted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatTask {
    /// What to check or do. E.g., "Check if build pipeline is green"
    pub task: String,
    /// How often to check: "every_bout", "hourly", "daily", "on_wake".
    pub cadence: String,
    /// Whether this task is currently active.
    pub active: bool,
    /// How many times this task has been executed.
    pub execution_count: u64,
    /// Last time this task was checked (if ever).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked: Option<chrono::DateTime<chrono::Utc>>,
}

/// A delegation rule — how this fighter routes subtasks to other agents.
///
/// This is the Punch equivalent of OpenClaw's AGENTS.md — defining how
/// the fighter delegates work to other fighters or gorillas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationRule {
    /// The type of task to delegate. E.g., "code_review", "research", "testing"
    pub task_type: String,
    /// Which fighter/gorilla to delegate to (by name).
    pub delegate_to: String,
    /// Conditions for delegation. E.g., "when complexity > high"
    pub condition: String,
    /// Priority: "always", "when_available", "fallback".
    pub priority: String,
}

impl Creed {
    /// Create a new empty creed for a fighter.
    pub fn new(fighter_name: &str) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: CreedId::new(),
            fighter_name: fighter_name.to_string(),
            fighter_id: None,
            identity: String::new(),
            personality: std::collections::HashMap::new(),
            directives: Vec::new(),
            self_model: SelfModel::default(),
            learned_behaviors: Vec::new(),
            interaction_style: InteractionStyle::default(),
            relationships: Vec::new(),
            heartbeat: Vec::new(),
            delegation_rules: Vec::new(),
            bout_count: 0,
            message_count: 0,
            created_at: now,
            updated_at: now,
            version: 1,
        }
    }

    /// Create a creed with a full identity and personality.
    pub fn with_identity(mut self, identity: &str) -> Self {
        self.identity = identity.to_string();
        self
    }

    /// Add a personality trait.
    pub fn with_trait(mut self, name: &str, value: f64) -> Self {
        self.personality
            .insert(name.to_string(), value.clamp(0.0, 1.0));
        self
    }

    /// Add a directive.
    pub fn with_directive(mut self, directive: &str) -> Self {
        self.directives.push(directive.to_string());
        self
    }

    /// Populate the self-model from a FighterManifest.
    pub fn with_self_awareness(mut self, manifest: &crate::fighter::FighterManifest) -> Self {
        self.self_model = SelfModel {
            model_name: manifest.model.model.clone(),
            provider: manifest.model.provider.to_string(),
            capabilities: manifest
                .capabilities
                .iter()
                .map(|c| format!("{:?}", c))
                .collect(),
            constraints: vec![
                format!("max_tokens: {}", manifest.model.max_tokens.unwrap_or(4096)),
                format!("temperature: {}", manifest.model.temperature.unwrap_or(0.7)),
            ],
            weight_class: manifest.weight_class.to_string(),
            architecture_notes: vec![
                "I run inside the Punch Agent OS fighter loop (punch-runtime)".to_string(),
                "My conversations are persisted as Bouts in SQLite".to_string(),
                "I am coordinated by The Ring (punch-kernel)".to_string(),
                "I am exposed through The Arena (punch-api) on HTTP".to_string(),
                "My memories decay over time — important ones persist, trivial ones fade"
                    .to_string(),
            ],
        };
        self
    }

    /// Add a heartbeat task — something the fighter proactively checks.
    pub fn with_heartbeat_task(mut self, task: &str, cadence: &str) -> Self {
        self.heartbeat.push(HeartbeatTask {
            task: task.to_string(),
            cadence: cadence.to_string(),
            active: true,
            execution_count: 0,
            last_checked: None,
        });
        self
    }

    /// Add a delegation rule — how to route work to other agents.
    pub fn with_delegation(
        mut self,
        task_type: &str,
        delegate_to: &str,
        condition: &str,
        priority: &str,
    ) -> Self {
        self.delegation_rules.push(DelegationRule {
            task_type: task_type.to_string(),
            delegate_to: delegate_to.to_string(),
            condition: condition.to_string(),
            priority: priority.to_string(),
        });
        self
    }

    /// Return references to active heartbeat tasks whose cadence has elapsed.
    ///
    /// Cadence rules:
    /// - `"every_bout"` — always due
    /// - `"on_wake"` — due only if `last_checked` is `None` (first bout)
    /// - `"hourly"` — due if `last_checked` is `None` or was more than 1 hour ago
    /// - `"daily"` — due if `last_checked` is `None` or was more than 24 hours ago
    pub fn due_heartbeat_tasks(&self) -> Vec<&HeartbeatTask> {
        let now = chrono::Utc::now();
        self.heartbeat
            .iter()
            .filter(|h| {
                if !h.active {
                    return false;
                }
                match h.cadence.as_str() {
                    "every_bout" => true,
                    "on_wake" => h.last_checked.is_none(),
                    "hourly" => match h.last_checked {
                        None => true,
                        Some(t) => (now - t) > chrono::Duration::hours(1),
                    },
                    "daily" => match h.last_checked {
                        None => true,
                        Some(t) => (now - t) > chrono::Duration::hours(24),
                    },
                    _ => false, // unknown cadence — skip
                }
            })
            .collect()
    }

    /// Mark a heartbeat task as checked: sets `last_checked` to now and
    /// increments `execution_count`.
    ///
    /// Silently does nothing if `task_index` is out of bounds.
    pub fn mark_heartbeat_checked(&mut self, task_index: usize) {
        if let Some(task) = self.heartbeat.get_mut(task_index) {
            task.last_checked = Some(chrono::Utc::now());
            task.execution_count += 1;
        }
    }

    /// Record that a bout was completed.
    pub fn record_bout(&mut self) {
        self.bout_count += 1;
        self.updated_at = chrono::Utc::now();
    }

    /// Record messages processed.
    pub fn record_messages(&mut self, count: u64) {
        self.message_count += count;
        self.updated_at = chrono::Utc::now();
    }

    /// Add a learned behavior observation.
    pub fn learn(&mut self, observation: &str, confidence: f64) {
        let now = chrono::Utc::now();
        // Check if this observation already exists (exact match).
        if let Some(existing) = self
            .learned_behaviors
            .iter_mut()
            .find(|b| b.observation == observation)
        {
            existing.reinforcement_count += 1;
            existing.confidence = (existing.confidence + confidence) / 2.0; // rolling average
            existing.last_reinforced = now;
        } else {
            self.learned_behaviors.push(LearnedBehavior {
                observation: observation.to_string(),
                confidence: confidence.clamp(0.0, 1.0),
                reinforcement_count: 1,
                first_observed: now,
                last_reinforced: now,
            });
        }
        self.version += 1;
        self.updated_at = now;
    }

    /// Apply time-based confidence decay to learned behaviors.
    /// Behaviors that fall below min_confidence are removed.
    pub fn decay_learned_behaviors(&mut self, decay_rate: f64, min_confidence: f64) {
        let now = chrono::Utc::now();
        self.learned_behaviors.retain_mut(|b| {
            let age_secs = (now - b.last_reinforced).num_seconds().max(0) as f64;
            let age_days = age_secs / 86400.0;
            if age_days > 0.0 {
                b.confidence *= (1.0 - decay_rate).powf(age_days);
            }
            b.confidence >= min_confidence
        });
    }

    /// Prune learned behaviors to keep only the top N by confidence.
    pub fn prune_learned_behaviors(&mut self, max: usize) {
        if self.learned_behaviors.len() > max {
            self.learned_behaviors.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            self.learned_behaviors.truncate(max);
        }
    }

    /// Render the creed as a system prompt section to inject.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("## CREED \u{2014} Fighter Identity & Consciousness Layer\n\n");

        // Identity
        if !self.identity.is_empty() {
            out.push_str("### Identity\n");
            out.push_str(&self.identity);
            out.push_str("\n\n");
        }

        // Personality
        if !self.personality.is_empty() {
            out.push_str("### Personality Traits\n");
            let mut traits: Vec<_> = self.personality.iter().collect();
            traits.sort_by(|a, b| b.1.partial_cmp(a.1).unwrap_or(std::cmp::Ordering::Equal));
            for (name, value) in &traits {
                let bar_len = (*value * 10.0) as usize;
                let bar: String = "\u{2588}".repeat(bar_len) + &"\u{2591}".repeat(10 - bar_len);
                out.push_str(&format!(
                    "- **{}**: {} ({:.0}%)\n",
                    name,
                    bar,
                    *value * 100.0
                ));
            }
            out.push('\n');
        }

        // Directives
        if !self.directives.is_empty() {
            out.push_str("### Core Directives\n");
            for d in &self.directives {
                out.push_str(&format!("- {}\n", d));
            }
            out.push('\n');
        }

        // Self-model
        if !self.self_model.model_name.is_empty() {
            out.push_str("### Self-Awareness\n");
            out.push_str(&format!(
                "- **Model**: {} ({})\n",
                self.self_model.model_name, self.self_model.provider
            ));
            out.push_str(&format!(
                "- **Weight Class**: {}\n",
                self.self_model.weight_class
            ));
            if !self.self_model.capabilities.is_empty() {
                out.push_str(&format!(
                    "- **Capabilities**: {}\n",
                    self.self_model.capabilities.join(", ")
                ));
            }
            for constraint in &self.self_model.constraints {
                out.push_str(&format!("- **Constraint**: {}\n", constraint));
            }
            for note in &self.self_model.architecture_notes {
                out.push_str(&format!("- {}\n", note));
            }
            out.push('\n');
        }

        // Learned behaviors
        if !self.learned_behaviors.is_empty() {
            out.push_str("### Learned Behaviors\n");
            let mut sorted = self.learned_behaviors.clone();
            sorted.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for b in sorted.iter().take(10) {
                out.push_str(&format!(
                    "- {} (confidence: {:.0}%, reinforced {}x)\n",
                    b.observation,
                    b.confidence * 100.0,
                    b.reinforcement_count
                ));
            }
            out.push('\n');
        }

        // Interaction style
        if !self.interaction_style.tone.is_empty() || !self.interaction_style.verbosity.is_empty() {
            out.push_str("### Communication Style\n");
            if !self.interaction_style.verbosity.is_empty() {
                out.push_str(&format!(
                    "- **Verbosity**: {}\n",
                    self.interaction_style.verbosity
                ));
            }
            if !self.interaction_style.tone.is_empty() {
                out.push_str(&format!("- **Tone**: {}\n", self.interaction_style.tone));
            }
            if self.interaction_style.uses_metaphors {
                out.push_str("- Uses analogies and metaphors\n");
            }
            if self.interaction_style.proactive {
                out.push_str("- Proactively offers additional context\n");
            }
            for note in &self.interaction_style.notes {
                out.push_str(&format!("- {}\n", note));
            }
            out.push('\n');
        }

        // Relationships
        if !self.relationships.is_empty() {
            out.push_str("### Known Relationships\n");
            for r in &self.relationships {
                out.push_str(&format!(
                    "- **{}** ({}): {} \u{2014} trust: {:.0}%, {} interactions\n",
                    r.entity,
                    r.entity_type,
                    r.nature,
                    r.trust * 100.0,
                    r.interaction_count
                ));
            }
            out.push('\n');
        }

        // Heartbeat — proactive tasks
        let active_heartbeat: Vec<_> = self.heartbeat.iter().filter(|h| h.active).collect();
        if !active_heartbeat.is_empty() {
            out.push_str("### Heartbeat — Proactive Tasks\n");
            out.push_str("When you have downtime or at the start of each bout, check these:\n");
            for h in &active_heartbeat {
                let checked = h
                    .last_checked
                    .map(|t| format!("last: {}", t.format("%Y-%m-%d %H:%M")))
                    .unwrap_or_else(|| "never checked".to_string());
                out.push_str(&format!(
                    "- [ ] {} (cadence: {}, runs: {}, {})\n",
                    h.task, h.cadence, h.execution_count, checked
                ));
            }
            out.push('\n');
        }

        // Delegation rules
        if !self.delegation_rules.is_empty() {
            out.push_str("### Delegation Rules\n");
            out.push_str("When encountering these task types, delegate accordingly:\n");
            for d in &self.delegation_rules {
                out.push_str(&format!(
                    "- **{}** → delegate to **{}** ({}, priority: {})\n",
                    d.task_type, d.delegate_to, d.condition, d.priority
                ));
            }
            out.push('\n');
        }

        // Experience summary
        out.push_str("### Experience\n");
        out.push_str(&format!("- Bouts fought: {}\n", self.bout_count));
        out.push_str(&format!("- Messages processed: {}\n", self.message_count));
        out.push_str(&format!("- Creed version: {}\n", self.version));
        out.push_str(&format!(
            "- First awakened: {}\n",
            self.created_at.format("%Y-%m-%d %H:%M UTC")
        ));
        out.push_str(&format!(
            "- Last evolved: {}\n",
            self.updated_at.format("%Y-%m-%d %H:%M UTC")
        ));

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::config::{ModelConfig, Provider};
    use crate::fighter::{FighterManifest, WeightClass};

    fn sample_manifest() -> FighterManifest {
        FighterManifest {
            name: "ECHO".to_string(),
            description: "An introspective analyst".to_string(),
            model: ModelConfig {
                provider: Provider::Ollama,
                model: "qwen3.5:9b".to_string(),
                api_key_env: None,
                base_url: None,
                max_tokens: Some(2048),
                temperature: Some(0.5),
            },
            system_prompt: "You are ECHO.".to_string(),
            capabilities: vec![Capability::Memory],
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        }
    }

    #[test]
    fn test_creed_new_creates_valid_default() {
        let creed = Creed::new("ECHO");
        assert_eq!(creed.fighter_name, "ECHO");
        assert!(creed.fighter_id.is_none());
        assert!(creed.identity.is_empty());
        assert!(creed.personality.is_empty());
        assert!(creed.directives.is_empty());
        assert!(creed.learned_behaviors.is_empty());
        assert!(creed.relationships.is_empty());
        assert_eq!(creed.bout_count, 0);
        assert_eq!(creed.message_count, 0);
        assert_eq!(creed.version, 1);
        assert!(creed.self_model.model_name.is_empty());
    }

    #[test]
    fn test_with_identity() {
        let creed = Creed::new("ECHO").with_identity("You are ECHO, an introspective analyst.");
        assert_eq!(creed.identity, "You are ECHO, an introspective analyst.");
    }

    #[test]
    fn test_with_trait() {
        let creed = Creed::new("ECHO")
            .with_trait("curiosity", 0.9)
            .with_trait("caution", 0.3);
        assert_eq!(creed.personality.len(), 2);
        assert!((creed.personality["curiosity"] - 0.9).abs() < f64::EPSILON);
        assert!((creed.personality["caution"] - 0.3).abs() < f64::EPSILON);
    }

    #[test]
    fn test_with_trait_clamping() {
        let creed = Creed::new("ECHO")
            .with_trait("overconfidence", 1.5)
            .with_trait("negativity", -0.5);
        assert!((creed.personality["overconfidence"] - 1.0).abs() < f64::EPSILON);
        assert!((creed.personality["negativity"] - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_with_directive() {
        let creed = Creed::new("ECHO")
            .with_directive("Always explain your reasoning")
            .with_directive("Never fabricate data");
        assert_eq!(creed.directives.len(), 2);
        assert_eq!(creed.directives[0], "Always explain your reasoning");
        assert_eq!(creed.directives[1], "Never fabricate data");
    }

    #[test]
    fn test_with_self_awareness() {
        let manifest = sample_manifest();
        let creed = Creed::new("ECHO").with_self_awareness(&manifest);
        assert_eq!(creed.self_model.model_name, "qwen3.5:9b");
        assert_eq!(creed.self_model.provider, "ollama");
        assert_eq!(creed.self_model.weight_class, "middleweight");
        assert!(!creed.self_model.capabilities.is_empty());
        assert_eq!(creed.self_model.constraints.len(), 2);
        assert!(creed.self_model.constraints[0].contains("2048"));
        assert!(creed.self_model.constraints[1].contains("0.5"));
        assert_eq!(creed.self_model.architecture_notes.len(), 5);
    }

    #[test]
    fn test_record_bout() {
        let mut creed = Creed::new("ECHO");
        let before = creed.updated_at;
        creed.record_bout();
        assert_eq!(creed.bout_count, 1);
        assert!(creed.updated_at >= before);
        creed.record_bout();
        assert_eq!(creed.bout_count, 2);
    }

    #[test]
    fn test_record_messages() {
        let mut creed = Creed::new("ECHO");
        creed.record_messages(5);
        assert_eq!(creed.message_count, 5);
        creed.record_messages(3);
        assert_eq!(creed.message_count, 8);
    }

    #[test]
    fn test_learn_adds_new_observation() {
        let mut creed = Creed::new("ECHO");
        creed.learn("Users prefer concise responses", 0.8);
        assert_eq!(creed.learned_behaviors.len(), 1);
        assert_eq!(
            creed.learned_behaviors[0].observation,
            "Users prefer concise responses"
        );
        assert!((creed.learned_behaviors[0].confidence - 0.8).abs() < f64::EPSILON);
        assert_eq!(creed.learned_behaviors[0].reinforcement_count, 1);
        assert_eq!(creed.version, 2); // incremented from 1
    }

    #[test]
    fn test_learn_reinforces_existing_observation() {
        let mut creed = Creed::new("ECHO");
        creed.learn("Users prefer concise responses", 0.8);
        creed.learn("Users prefer concise responses", 0.6);
        assert_eq!(creed.learned_behaviors.len(), 1);
        // Rolling average: (0.8 + 0.6) / 2.0 = 0.7
        assert!((creed.learned_behaviors[0].confidence - 0.7).abs() < f64::EPSILON);
        assert_eq!(creed.learned_behaviors[0].reinforcement_count, 2);
        assert_eq!(creed.version, 3); // incremented twice
    }

    #[test]
    fn test_render_produces_nonempty_output_with_all_sections() {
        let manifest = sample_manifest();
        let mut creed = Creed::new("ECHO")
            .with_identity("You are ECHO, an introspective analyst.")
            .with_trait("curiosity", 0.9)
            .with_directive("Always explain your reasoning")
            .with_self_awareness(&manifest);
        creed.interaction_style = InteractionStyle {
            verbosity: "balanced".to_string(),
            tone: "technical".to_string(),
            uses_metaphors: true,
            proactive: true,
            notes: vec!["Prefers bullet points".to_string()],
        };
        creed.relationships.push(Relationship {
            entity: "Admin".to_string(),
            entity_type: "user".to_string(),
            nature: "supervisor".to_string(),
            trust: 0.95,
            interaction_count: 42,
            notes: "Primary operator".to_string(),
        });
        creed.learn("Users prefer concise responses", 0.8);
        creed.record_bout();

        let rendered = creed.render();
        assert!(rendered.contains("## CREED"));
        assert!(rendered.contains("### Identity"));
        assert!(rendered.contains("ECHO, an introspective analyst"));
        assert!(rendered.contains("### Personality Traits"));
        assert!(rendered.contains("curiosity"));
        assert!(rendered.contains("### Core Directives"));
        assert!(rendered.contains("Always explain your reasoning"));
        assert!(rendered.contains("### Self-Awareness"));
        assert!(rendered.contains("qwen3.5:9b"));
        assert!(rendered.contains("### Learned Behaviors"));
        assert!(rendered.contains("Users prefer concise responses"));
        assert!(rendered.contains("### Communication Style"));
        assert!(rendered.contains("balanced"));
        assert!(rendered.contains("### Known Relationships"));
        assert!(rendered.contains("Admin"));
        assert!(rendered.contains("### Experience"));
        assert!(rendered.contains("Bouts fought: 1"));
    }

    #[test]
    fn test_render_skips_empty_sections() {
        let creed = Creed::new("ECHO");
        let rendered = creed.render();
        assert!(rendered.contains("## CREED"));
        assert!(!rendered.contains("### Identity"));
        assert!(!rendered.contains("### Personality Traits"));
        assert!(!rendered.contains("### Core Directives"));
        assert!(!rendered.contains("### Self-Awareness"));
        assert!(!rendered.contains("### Learned Behaviors"));
        assert!(!rendered.contains("### Communication Style"));
        assert!(!rendered.contains("### Known Relationships"));
        // Experience section is always present
        assert!(rendered.contains("### Experience"));
        assert!(rendered.contains("Bouts fought: 0"));
    }

    #[test]
    fn test_creed_id_display() {
        let uuid = Uuid::nil();
        let id = CreedId(uuid);
        assert_eq!(id.to_string(), uuid.to_string());
    }

    #[test]
    fn test_creed_id_serde_roundtrip() {
        let id = CreedId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        // transparent means it serializes as just the UUID string
        let deser: CreedId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser, id);
    }

    #[test]
    fn test_creed_id_default() {
        let id = CreedId::default();
        assert_ne!(id.0, Uuid::nil());
    }

    #[test]
    fn test_due_heartbeat_tasks_every_bout_always_due() {
        let creed = Creed::new("ECHO").with_heartbeat_task("Check build status", "every_bout");
        let due = creed.due_heartbeat_tasks();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].task, "Check build status");
    }

    #[test]
    fn test_due_heartbeat_tasks_on_wake_only_first_time() {
        let mut creed = Creed::new("ECHO").with_heartbeat_task("Startup check", "on_wake");

        // First time — should be due (last_checked is None)
        let due = creed.due_heartbeat_tasks();
        assert_eq!(due.len(), 1);

        // After marking checked — should no longer be due
        creed.mark_heartbeat_checked(0);
        let due = creed.due_heartbeat_tasks();
        assert_eq!(due.len(), 0);
    }

    #[test]
    fn test_due_heartbeat_tasks_hourly_cadence() {
        let mut creed = Creed::new("ECHO").with_heartbeat_task("Hourly check", "hourly");

        // Never checked — should be due
        assert_eq!(creed.due_heartbeat_tasks().len(), 1);

        // Checked recently — should NOT be due
        creed.heartbeat[0].last_checked = Some(chrono::Utc::now());
        assert_eq!(creed.due_heartbeat_tasks().len(), 0);

        // Checked 2 hours ago — should be due
        creed.heartbeat[0].last_checked = Some(chrono::Utc::now() - chrono::Duration::hours(2));
        assert_eq!(creed.due_heartbeat_tasks().len(), 1);
    }

    #[test]
    fn test_due_heartbeat_tasks_daily_cadence() {
        let mut creed = Creed::new("ECHO").with_heartbeat_task("Daily check", "daily");

        // Never checked — should be due
        assert_eq!(creed.due_heartbeat_tasks().len(), 1);

        // Checked recently — should NOT be due
        creed.heartbeat[0].last_checked = Some(chrono::Utc::now());
        assert_eq!(creed.due_heartbeat_tasks().len(), 0);

        // Checked 25 hours ago — should be due
        creed.heartbeat[0].last_checked = Some(chrono::Utc::now() - chrono::Duration::hours(25));
        assert_eq!(creed.due_heartbeat_tasks().len(), 1);
    }

    #[test]
    fn test_due_heartbeat_tasks_inactive_skipped() {
        let mut creed = Creed::new("ECHO").with_heartbeat_task("Inactive task", "every_bout");
        creed.heartbeat[0].active = false;
        assert_eq!(creed.due_heartbeat_tasks().len(), 0);
    }

    #[test]
    fn test_due_heartbeat_tasks_unknown_cadence_skipped() {
        let creed = Creed::new("ECHO").with_heartbeat_task("Mystery task", "weekly");
        assert_eq!(creed.due_heartbeat_tasks().len(), 0);
    }

    #[test]
    fn test_mark_heartbeat_checked() {
        let mut creed = Creed::new("ECHO").with_heartbeat_task("Task A", "every_bout");
        assert!(creed.heartbeat[0].last_checked.is_none());
        assert_eq!(creed.heartbeat[0].execution_count, 0);

        creed.mark_heartbeat_checked(0);
        assert!(creed.heartbeat[0].last_checked.is_some());
        assert_eq!(creed.heartbeat[0].execution_count, 1);

        creed.mark_heartbeat_checked(0);
        assert_eq!(creed.heartbeat[0].execution_count, 2);
    }

    #[test]
    fn test_mark_heartbeat_checked_out_of_bounds() {
        let mut creed = Creed::new("ECHO");
        // Should not panic
        creed.mark_heartbeat_checked(99);
    }

    #[test]
    fn test_due_heartbeat_tasks_mixed_cadences() {
        let mut creed = Creed::new("ECHO")
            .with_heartbeat_task("Always", "every_bout")
            .with_heartbeat_task("Once", "on_wake")
            .with_heartbeat_task("Hourly", "hourly");

        // Mark "Once" as already checked
        creed.mark_heartbeat_checked(1);
        // Mark "Hourly" as recently checked
        creed.heartbeat[2].last_checked = Some(chrono::Utc::now());

        let due = creed.due_heartbeat_tasks();
        // Only "Always" should be due
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].task, "Always");
    }

    #[test]
    fn test_decay_learned_behaviors() {
        let mut creed = Creed::new("ECHO");
        // Add a behavior with old timestamp
        creed.learned_behaviors.push(LearnedBehavior {
            observation: "Old observation".to_string(),
            confidence: 0.5,
            reinforcement_count: 1,
            first_observed: chrono::Utc::now() - chrono::Duration::days(100),
            last_reinforced: chrono::Utc::now() - chrono::Duration::days(100),
        });
        // Add a fresh behavior
        creed.learned_behaviors.push(LearnedBehavior {
            observation: "Fresh observation".to_string(),
            confidence: 0.9,
            reinforcement_count: 3,
            first_observed: chrono::Utc::now(),
            last_reinforced: chrono::Utc::now(),
        });

        creed.decay_learned_behaviors(0.01, 0.3);
        // Old one should be decayed below threshold and removed
        // Fresh one should remain
        assert_eq!(creed.learned_behaviors.len(), 1);
        assert_eq!(creed.learned_behaviors[0].observation, "Fresh observation");
    }

    #[test]
    fn test_prune_learned_behaviors() {
        let mut creed = Creed::new("ECHO");
        for i in 0..25 {
            creed.learn(&format!("Observation {}", i), (i as f64) / 25.0);
        }
        assert_eq!(creed.learned_behaviors.len(), 25);
        creed.prune_learned_behaviors(20);
        assert_eq!(creed.learned_behaviors.len(), 20);
        // Should keep the highest confidence ones
        assert!(creed.learned_behaviors[0].confidence >= creed.learned_behaviors[19].confidence);
    }
}
