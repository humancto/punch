//! Tool Policy Engine — the ring regulations for move control.
//!
//! A deny-wins, multi-layer policy engine that controls which moves (tools)
//! fighters can throw during a bout. Policies are evaluated by priority, and
//! if ANY matching rule denies a move, the overall decision is denial —
//! the strictest referee always wins.
//!
//! ## Architecture
//!
//! - [`PolicyEffect`] determines whether a rule allows or denies a move
//! - [`PolicyScope`] sets the blast radius of a rule (global, fighter, etc.)
//! - [`PolicyCondition`] adds time, rate-limit, or capability constraints
//! - [`PolicyRule`] combines patterns, effects, and conditions into a fight rule
//! - [`ToolPolicyEngine`] evaluates all matching rules with deny-wins semantics
//! - [`PolicyDecision`] reports the outcome and which rules matched

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Policy effect
// ---------------------------------------------------------------------------

/// The effect of a policy rule — does it let the move through or block it?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyEffect {
    /// The move is allowed — fight on.
    Allow,
    /// The move is denied — stand down, fighter.
    Deny,
}

// ---------------------------------------------------------------------------
// Policy scope
// ---------------------------------------------------------------------------

/// What level a policy rule applies at — from ring-wide regulations down to
/// individual move restrictions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "value")]
pub enum PolicyScope {
    /// Applies to every fighter in the ring.
    Global,
    /// Applies to a specific fighter by name.
    Fighter(String),
    /// Applies to fighters in a specific weight class.
    WeightClass(String),
    /// Applies to a specific tool (move).
    Tool(String),
}

// ---------------------------------------------------------------------------
// Policy condition
// ---------------------------------------------------------------------------

/// Additional conditions that must be met for a policy rule to be active.
/// These are the fine print in the fight contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum PolicyCondition {
    /// Rule is only active during certain hours (UTC). The fighter can only
    /// throw this move during the scheduled bout window.
    TimeWindow {
        /// Start hour (0-23, UTC).
        start_hour: u8,
        /// End hour (0-23, UTC). If end < start, wraps past midnight.
        end_hour: u8,
    },
    /// Rate limit — maximum invocations within a rolling time window.
    /// Prevents a fighter from spamming the same move.
    MaxInvocations {
        /// Maximum number of invocations allowed.
        count: u32,
        /// Rolling window duration in seconds.
        window_secs: u64,
    },
    /// The fighter must possess this capability to match this rule.
    /// Like requiring a certain belt rank to enter the ring.
    RequireCapability {
        /// The capability name the fighter must hold.
        capability: String,
    },
}

// ---------------------------------------------------------------------------
// Policy rule
// ---------------------------------------------------------------------------

/// A single fight rule that controls which moves fighters can throw.
///
/// Rules are matched by glob patterns against tool names and fighter names,
/// and can carry additional conditions (time windows, rate limits, capabilities).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Human-readable name for this ring regulation.
    pub name: String,
    /// Whether this rule allows or denies the move.
    pub effect: PolicyEffect,
    /// Glob patterns matching tool (move) names. E.g. `"shell_*"`, `"file_write"`, `"*"`.
    pub tool_patterns: Vec<String>,
    /// Glob patterns matching fighter names. E.g. `"*"`, `"worker-*"`.
    pub fighter_patterns: Vec<String>,
    /// What level this rule applies at.
    pub scope: PolicyScope,
    /// Priority — higher priority rules are evaluated first. Default is 0.
    pub priority: i32,
    /// Additional conditions that must be met for this rule to be active.
    pub conditions: Vec<PolicyCondition>,
    /// Why this rule exists — the referee's rationale.
    pub description: String,
}

// ---------------------------------------------------------------------------
// Policy decision
// ---------------------------------------------------------------------------

/// The outcome of evaluating all ring regulations for a move.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    /// Whether the move is allowed.
    pub allowed: bool,
    /// Names of all rules that matched.
    pub matching_rules: Vec<String>,
    /// If denied, the reason the referee blocked the move.
    pub denial_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Glob helper
// ---------------------------------------------------------------------------

/// Check if a value matches any glob pattern in the list.
/// Returns `true` if at least one pattern matches, like checking if a move
/// is in the fighter's approved moveset.
pub fn glob_list_matches(patterns: &[String], value: &str) -> bool {
    for pattern_str in patterns {
        if pattern_str == "*" || pattern_str == "**" {
            return true;
        }
        if let Ok(pattern) = glob::Pattern::new(pattern_str)
            && pattern.matches(value)
        {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tool Policy Engine
// ---------------------------------------------------------------------------

/// The ring regulation engine — evaluates fight rules to decide whether a
/// fighter can throw a given move.
///
/// Uses deny-wins semantics: if ANY matching rule denies the move, the
/// overall decision is denial. The strictest referee always prevails.
///
/// Thread-safe invocation tracking via `DashMap` supports concurrent bouts.
#[derive(Debug)]
pub struct ToolPolicyEngine {
    /// All registered fight rules.
    rules: Vec<PolicyRule>,
    /// Per-key invocation timestamps for rate-limit conditions.
    /// Key format: `"{rule_name}:{fighter_name}:{tool_name}"`.
    invocation_counts: DashMap<String, Vec<DateTime<Utc>>>,
}

impl ToolPolicyEngine {
    /// Create a new engine with no rules — an anything-goes ring.
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            invocation_counts: DashMap::new(),
        }
    }

    /// Add a fight rule to the ring regulations.
    pub fn add_rule(&mut self, rule: PolicyRule) {
        self.rules.push(rule);
    }

    /// Remove a fight rule by name. Returns `true` if a rule was removed.
    pub fn remove_rule(&mut self, name: &str) -> bool {
        let before = self.rules.len();
        self.rules.retain(|r| r.name != name);
        self.rules.len() < before
    }

    /// Evaluate all matching rules for a tool invocation by a fighter.
    ///
    /// The referee checks every applicable rule, sorted by priority (highest
    /// first). **Deny wins**: if any matching rule denies the move, the
    /// overall result is denial regardless of allow rules.
    ///
    /// If no rules match at all, the default is to allow — permissive by
    /// default, like an unsanctioned bout.
    pub fn evaluate(
        &self,
        tool_name: &str,
        fighter_name: &str,
        capabilities: &[String],
    ) -> PolicyDecision {
        let matching = self.matching_rules(tool_name, fighter_name);

        if matching.is_empty() {
            return PolicyDecision {
                allowed: true,
                matching_rules: Vec::new(),
                denial_reason: None,
            };
        }

        // Sort by priority descending (higher priority first).
        let mut sorted: Vec<&PolicyRule> = matching;
        sorted.sort_by(|a, b| b.priority.cmp(&a.priority));

        let mut matched_names = Vec::new();
        let mut denied = false;
        let mut denial_reason: Option<String> = None;

        let now = Utc::now();

        for rule in &sorted {
            // Check conditions — all must pass for the rule to be active.
            if !self.check_conditions(rule, tool_name, fighter_name, capabilities, now) {
                continue;
            }

            matched_names.push(rule.name.clone());

            if rule.effect == PolicyEffect::Deny {
                denied = true;
                if denial_reason.is_none() {
                    denial_reason = Some(format!(
                        "denied by rule '{}': {}",
                        rule.name, rule.description
                    ));
                }
            }
        }

        // Record invocation for rate-limiting (only if allowed).
        if !denied {
            for rule in &sorted {
                for cond in &rule.conditions {
                    if let PolicyCondition::MaxInvocations { .. } = cond {
                        let key = format!("{}:{}:{}", rule.name, fighter_name, tool_name);
                        self.invocation_counts.entry(key).or_default().push(now);
                    }
                }
            }
        }

        PolicyDecision {
            allowed: !denied,
            matching_rules: matched_names,
            denial_reason,
        }
    }

    /// Get all currently registered fight rules.
    pub fn rules(&self) -> &[PolicyRule] {
        &self.rules
    }

    /// Get all rules whose tool and fighter patterns match the given names.
    /// Does NOT check conditions — just pattern matching.
    pub fn matching_rules(&self, tool_name: &str, fighter_name: &str) -> Vec<&PolicyRule> {
        self.rules
            .iter()
            .filter(|rule| {
                glob_list_matches(&rule.tool_patterns, tool_name)
                    && glob_list_matches(&rule.fighter_patterns, fighter_name)
            })
            .collect()
    }

    /// Check whether all conditions on a rule are satisfied.
    fn check_conditions(
        &self,
        rule: &PolicyRule,
        tool_name: &str,
        fighter_name: &str,
        capabilities: &[String],
        now: DateTime<Utc>,
    ) -> bool {
        for condition in &rule.conditions {
            match condition {
                PolicyCondition::TimeWindow {
                    start_hour,
                    end_hour,
                } => {
                    let current_hour = now.format("%H").to_string();
                    let hour: u8 = current_hour.parse().unwrap_or(0);
                    let in_window = if start_hour <= end_hour {
                        // Normal range, e.g. 9..17
                        hour >= *start_hour && hour < *end_hour
                    } else {
                        // Wraps past midnight, e.g. 22..6
                        hour >= *start_hour || hour < *end_hour
                    };
                    if !in_window {
                        return false;
                    }
                }
                PolicyCondition::MaxInvocations { count, window_secs } => {
                    let key = format!("{}:{}:{}", rule.name, fighter_name, tool_name);
                    let cutoff = now - chrono::Duration::seconds(*window_secs as i64);
                    if let Some(timestamps) = self.invocation_counts.get(&key) {
                        let recent_count =
                            timestamps.iter().filter(|t| **t >= cutoff).count() as u32;
                        if recent_count < *count {
                            // Under the limit — condition not met (rule doesn't fire).
                            // For a Deny rule with MaxInvocations, the deny only kicks
                            // in when the limit is reached.
                            return false;
                        }
                    } else {
                        // No invocations recorded — under the limit.
                        return false;
                    }
                }
                PolicyCondition::RequireCapability { capability } => {
                    if !capabilities.contains(capability) {
                        return false;
                    }
                }
            }
        }
        true
    }
}

impl Default for ToolPolicyEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Default safety rules
// ---------------------------------------------------------------------------

/// Returns a set of common-sense ring regulations — the standard safety
/// rulebook that every well-run bout should start with.
///
/// - Deny `shell_exec` for readonly fighters
/// - Deny `file_write` for readonly fighters
/// - Allow all moves for admin fighters
/// - Deny `agent_spawn` by default (low priority, overridable)
pub fn default_safety_rules() -> Vec<PolicyRule> {
    vec![
        PolicyRule {
            name: "deny-shell-readonly".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["shell_exec".to_string()],
            fighter_patterns: vec!["*-readonly".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: Vec::new(),
            description: "Readonly fighters cannot execute shell commands".to_string(),
        },
        PolicyRule {
            name: "deny-filewrite-readonly".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["file_write".to_string()],
            fighter_patterns: vec!["*-readonly".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: Vec::new(),
            description: "Readonly fighters cannot write files".to_string(),
        },
        PolicyRule {
            name: "allow-all-admin".to_string(),
            effect: PolicyEffect::Allow,
            tool_patterns: vec!["*".to_string()],
            fighter_patterns: vec!["*-admin".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: Vec::new(),
            description: "Admin fighters have unrestricted access to all moves".to_string(),
        },
        PolicyRule {
            name: "deny-agent-spawn-default".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["agent_spawn".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: -10,
            conditions: Vec::new(),
            description:
                "Agent spawning is denied by default — override with a higher-priority allow rule"
                    .to_string(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Basic allow: no rules = allow all --

    #[test]
    fn test_no_rules_allows_all() {
        let engine = ToolPolicyEngine::new();
        let decision = engine.evaluate("shell_exec", "worker-1", &[]);
        assert!(decision.allowed);
        assert!(decision.matching_rules.is_empty());
        assert!(decision.denial_reason.is_none());
    }

    // -- Basic deny: deny rule blocks tool --

    #[test]
    fn test_deny_rule_blocks_tool() {
        let mut engine = ToolPolicyEngine::new();
        engine.add_rule(PolicyRule {
            name: "block-shell".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["shell_exec".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: Vec::new(),
            description: "No shell access in the ring".to_string(),
        });

        let decision = engine.evaluate("shell_exec", "worker-1", &[]);
        assert!(!decision.allowed);
        assert!(decision.denial_reason.is_some());
    }

    // -- Deny wins: allow + deny rules, deny takes precedence --

    #[test]
    fn test_deny_wins_over_allow() {
        let mut engine = ToolPolicyEngine::new();
        engine.add_rule(PolicyRule {
            name: "allow-all".to_string(),
            effect: PolicyEffect::Allow,
            tool_patterns: vec!["*".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 100,
            conditions: Vec::new(),
            description: "Allow everything".to_string(),
        });
        engine.add_rule(PolicyRule {
            name: "deny-shell".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["shell_exec".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: Vec::new(),
            description: "But not shell".to_string(),
        });

        let decision = engine.evaluate("shell_exec", "worker-1", &[]);
        assert!(
            !decision.allowed,
            "deny must win even when allow has higher priority"
        );
        assert!(decision.matching_rules.contains(&"allow-all".to_string()));
        assert!(decision.matching_rules.contains(&"deny-shell".to_string()));
    }

    // -- Priority ordering: higher priority evaluated first --

    #[test]
    fn test_priority_ordering() {
        let mut engine = ToolPolicyEngine::new();
        engine.add_rule(PolicyRule {
            name: "low-priority".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["*".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: -10,
            conditions: Vec::new(),
            description: "Low priority deny".to_string(),
        });
        engine.add_rule(PolicyRule {
            name: "high-priority".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["*".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 100,
            conditions: Vec::new(),
            description: "High priority deny".to_string(),
        });

        let decision = engine.evaluate("file_read", "worker-1", &[]);
        assert!(!decision.allowed);
        // High-priority rule should be listed first in matching_rules.
        assert_eq!(decision.matching_rules[0], "high-priority");
        assert_eq!(decision.matching_rules[1], "low-priority");
    }

    // -- Glob matching: "shell_*" matches "shell_exec" but not "file_read" --

    #[test]
    fn test_glob_tool_pattern_matching() {
        let mut engine = ToolPolicyEngine::new();
        engine.add_rule(PolicyRule {
            name: "deny-shell-star".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["shell_*".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: Vec::new(),
            description: "Deny all shell moves".to_string(),
        });

        let shell_decision = engine.evaluate("shell_exec", "worker-1", &[]);
        assert!(
            !shell_decision.allowed,
            "shell_exec should be denied by shell_*"
        );

        let file_decision = engine.evaluate("file_read", "worker-1", &[]);
        assert!(file_decision.allowed, "file_read should not match shell_*");
    }

    // -- Glob matching: "*" matches everything --

    #[test]
    fn test_glob_wildcard_matches_everything() {
        let mut engine = ToolPolicyEngine::new();
        engine.add_rule(PolicyRule {
            name: "deny-all".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["*".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: Vec::new(),
            description: "Lockdown".to_string(),
        });

        for tool in &["file_read", "shell_exec", "web_fetch", "agent_spawn"] {
            let decision = engine.evaluate(tool, "any-fighter", &[]);
            assert!(!decision.allowed, "{} should be denied by wildcard", tool);
        }
    }

    // -- Fighter pattern matching: "worker-*" matches "worker-1" but not "admin-1" --

    #[test]
    fn test_fighter_pattern_matching() {
        let mut engine = ToolPolicyEngine::new();
        engine.add_rule(PolicyRule {
            name: "deny-workers".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["*".to_string()],
            fighter_patterns: vec!["worker-*".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: Vec::new(),
            description: "Workers cannot use any tools".to_string(),
        });

        let worker_decision = engine.evaluate("file_read", "worker-1", &[]);
        assert!(!worker_decision.allowed, "worker-1 should match worker-*");

        let admin_decision = engine.evaluate("file_read", "admin-1", &[]);
        assert!(admin_decision.allowed, "admin-1 should not match worker-*");
    }

    // -- TimeWindow condition: inside window = active, outside = inactive --

    #[test]
    fn test_time_window_condition() {
        let mut engine = ToolPolicyEngine::new();
        let current_hour = Utc::now()
            .format("%H")
            .to_string()
            .parse::<u8>()
            .unwrap_or(0);

        // Create a window that includes the current hour.
        let start = current_hour;
        let end = if current_hour < 23 {
            current_hour + 2
        } else {
            1
        };

        engine.add_rule(PolicyRule {
            name: "deny-in-window".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["*".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: vec![PolicyCondition::TimeWindow {
                start_hour: start,
                end_hour: end,
            }],
            description: "Deny during active window".to_string(),
        });

        // Current time is inside the window, so the deny rule should be active.
        let decision = engine.evaluate("file_read", "worker-1", &[]);
        assert!(!decision.allowed, "should be denied inside time window");

        // Now create an engine with a window that excludes the current hour.
        let mut engine2 = ToolPolicyEngine::new();
        let outside_start = (current_hour + 3) % 24;
        let outside_end = (current_hour + 5) % 24;

        engine2.add_rule(PolicyRule {
            name: "deny-outside-window".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["*".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: vec![PolicyCondition::TimeWindow {
                start_hour: outside_start,
                end_hour: outside_end,
            }],
            description: "Deny during a future window".to_string(),
        });

        let decision2 = engine2.evaluate("file_read", "worker-1", &[]);
        assert!(decision2.allowed, "should be allowed outside time window");
    }

    // -- MaxInvocations condition: under limit = allow, over limit = deny --

    #[test]
    fn test_max_invocations_condition() {
        let mut engine = ToolPolicyEngine::new();
        engine.add_rule(PolicyRule {
            name: "rate-limit-shell".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["shell_exec".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: vec![PolicyCondition::MaxInvocations {
                count: 2,
                window_secs: 3600,
            }],
            description: "Rate limit shell to 2 per hour".to_string(),
        });

        // First two calls: under the limit, deny condition not met, so allowed.
        let d1 = engine.evaluate("shell_exec", "worker-1", &[]);
        assert!(
            d1.allowed,
            "first call should be allowed (under rate limit)"
        );

        let d2 = engine.evaluate("shell_exec", "worker-1", &[]);
        assert!(
            d2.allowed,
            "second call should be allowed (under rate limit)"
        );

        // Third call: at the limit, deny condition now met.
        let d3 = engine.evaluate("shell_exec", "worker-1", &[]);
        assert!(
            !d3.allowed,
            "third call should be denied (rate limit reached)"
        );
    }

    // -- RequireCapability condition: fighter with capability passes, without fails --

    #[test]
    fn test_require_capability_condition() {
        let mut engine = ToolPolicyEngine::new();
        engine.add_rule(PolicyRule {
            name: "deny-without-cap".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["dangerous_tool".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: vec![PolicyCondition::RequireCapability {
                capability: "elevated_access".to_string(),
            }],
            description: "Deny dangerous_tool unless fighter has elevated_access".to_string(),
        });

        // Fighter WITH the capability: the condition is met, so the deny fires.
        let with_cap = engine.evaluate(
            "dangerous_tool",
            "worker-1",
            &["elevated_access".to_string()],
        );
        assert!(
            !with_cap.allowed,
            "fighter with capability matches the deny rule's condition"
        );

        // Fighter WITHOUT the capability: the condition fails, rule doesn't fire.
        let without_cap = engine.evaluate("dangerous_tool", "worker-1", &[]);
        assert!(
            without_cap.allowed,
            "fighter without capability doesn't match the deny rule's condition"
        );
    }

    // -- Multiple rules with different scopes --

    #[test]
    fn test_multiple_rules_different_scopes() {
        let mut engine = ToolPolicyEngine::new();
        engine.add_rule(PolicyRule {
            name: "global-allow".to_string(),
            effect: PolicyEffect::Allow,
            tool_patterns: vec!["*".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: Vec::new(),
            description: "Global allow".to_string(),
        });
        engine.add_rule(PolicyRule {
            name: "fighter-deny".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["shell_exec".to_string()],
            fighter_patterns: vec!["restricted-fighter".to_string()],
            scope: PolicyScope::Fighter("restricted-fighter".to_string()),
            priority: 10,
            conditions: Vec::new(),
            description: "Fighter-specific deny".to_string(),
        });
        engine.add_rule(PolicyRule {
            name: "tool-deny".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["file_delete".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Tool("file_delete".to_string()),
            priority: 5,
            conditions: Vec::new(),
            description: "Tool-specific deny".to_string(),
        });

        // restricted-fighter + shell_exec = denied (fighter-specific rule).
        let d1 = engine.evaluate("shell_exec", "restricted-fighter", &[]);
        assert!(!d1.allowed);

        // restricted-fighter + file_read = allowed (only global-allow matches).
        let d2 = engine.evaluate("file_read", "restricted-fighter", &[]);
        assert!(d2.allowed);

        // any fighter + file_delete = denied (tool-specific rule).
        let d3 = engine.evaluate("file_delete", "worker-1", &[]);
        assert!(!d3.allowed);

        // any fighter + file_read = allowed.
        let d4 = engine.evaluate("file_read", "worker-1", &[]);
        assert!(d4.allowed);
    }

    // -- Remove rule by name --

    #[test]
    fn test_remove_rule() {
        let mut engine = ToolPolicyEngine::new();
        engine.add_rule(PolicyRule {
            name: "deny-shell".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["shell_exec".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: Vec::new(),
            description: "No shell".to_string(),
        });

        assert!(!engine.evaluate("shell_exec", "worker-1", &[]).allowed);

        let removed = engine.remove_rule("deny-shell");
        assert!(removed);
        assert!(engine.evaluate("shell_exec", "worker-1", &[]).allowed);

        // Removing a non-existent rule returns false.
        let removed_again = engine.remove_rule("deny-shell");
        assert!(!removed_again);
    }

    // -- default_safety_rules returns expected rules --

    #[test]
    fn test_default_safety_rules() {
        let rules = default_safety_rules();
        assert_eq!(rules.len(), 4);

        let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"deny-shell-readonly"));
        assert!(names.contains(&"deny-filewrite-readonly"));
        assert!(names.contains(&"allow-all-admin"));
        assert!(names.contains(&"deny-agent-spawn-default"));

        // Verify the default rules work correctly in an engine.
        let mut engine = ToolPolicyEngine::new();
        for rule in rules {
            engine.add_rule(rule);
        }

        // Readonly fighters should be denied shell and file_write.
        assert!(!engine.evaluate("shell_exec", "bot-readonly", &[]).allowed);
        assert!(!engine.evaluate("file_write", "bot-readonly", &[]).allowed);

        // Admin fighters can use agent_spawn (allow-all-admin matches,
        // but deny-agent-spawn-default also matches — deny wins).
        let admin_spawn = engine.evaluate("agent_spawn", "super-admin", &[]);
        assert!(!admin_spawn.allowed, "deny wins even for admin");

        // agent_spawn is denied for everyone by default.
        assert!(!engine.evaluate("agent_spawn", "worker-1", &[]).allowed);
    }

    // -- PolicyDecision includes matching rule names --

    #[test]
    fn test_policy_decision_includes_matching_rule_names() {
        let mut engine = ToolPolicyEngine::new();
        engine.add_rule(PolicyRule {
            name: "rule-alpha".to_string(),
            effect: PolicyEffect::Allow,
            tool_patterns: vec!["file_*".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 10,
            conditions: Vec::new(),
            description: "Alpha rule".to_string(),
        });
        engine.add_rule(PolicyRule {
            name: "rule-beta".to_string(),
            effect: PolicyEffect::Allow,
            tool_patterns: vec!["*".to_string()],
            fighter_patterns: vec!["*".to_string()],
            scope: PolicyScope::Global,
            priority: 0,
            conditions: Vec::new(),
            description: "Beta rule".to_string(),
        });

        let decision = engine.evaluate("file_read", "worker-1", &[]);
        assert!(decision.allowed);
        assert!(decision.matching_rules.contains(&"rule-alpha".to_string()));
        assert!(decision.matching_rules.contains(&"rule-beta".to_string()));
    }

    // -- Serialization round-trip of PolicyRule --

    #[test]
    fn test_policy_rule_serialization_roundtrip() {
        let rule = PolicyRule {
            name: "test-rule".to_string(),
            effect: PolicyEffect::Deny,
            tool_patterns: vec!["shell_*".to_string(), "file_write".to_string()],
            fighter_patterns: vec!["worker-*".to_string()],
            scope: PolicyScope::Fighter("worker-1".to_string()),
            priority: 42,
            conditions: vec![
                PolicyCondition::TimeWindow {
                    start_hour: 9,
                    end_hour: 17,
                },
                PolicyCondition::MaxInvocations {
                    count: 10,
                    window_secs: 3600,
                },
                PolicyCondition::RequireCapability {
                    capability: "admin".to_string(),
                },
            ],
            description: "A test fight rule".to_string(),
        };

        let json = serde_json::to_string(&rule).expect("serialization failed");
        let deserialized: PolicyRule = serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(deserialized.name, rule.name);
        assert_eq!(deserialized.effect, rule.effect);
        assert_eq!(deserialized.tool_patterns, rule.tool_patterns);
        assert_eq!(deserialized.fighter_patterns, rule.fighter_patterns);
        assert_eq!(deserialized.scope, rule.scope);
        assert_eq!(deserialized.priority, rule.priority);
        assert_eq!(deserialized.conditions.len(), 3);
        assert_eq!(deserialized.description, rule.description);
    }
}
