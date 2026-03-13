//! Policy-based tool gating — the referee system.
//!
//! Before a fighter can throw a move (execute a tool), the referee checks the
//! ring rules (policies). Depending on the risk level and configured policies,
//! a move may be allowed, denied outright, or held pending approval from a
//! cornerman (human operator).
//!
//! ## Architecture
//!
//! - [`RiskLevel`] classifies the danger of a tool action
//! - [`ApprovalPolicy`] defines rules mapping tool patterns to risk levels
//! - [`PolicyEngine`] evaluates tool calls against policies and delegates to
//!   an [`ApprovalHandler`] when human approval is required
//! - [`AutoApproveHandler`] and [`DenyAllHandler`] provide default handlers
//!   for headless/dev and locked-down modes respectively

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::PunchResult;
use crate::fighter::FighterId;

// ---------------------------------------------------------------------------
// Risk classification
// ---------------------------------------------------------------------------

/// Risk level assigned to a tool action, from a light jab to a knockout blow.
///
/// Higher risk levels demand more scrutiny before a fighter is allowed to
/// throw the move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// Safe, read-only operations — shadow boxing.
    Low,
    /// Operations with limited side effects — sparring.
    Medium,
    /// Destructive or sensitive operations — a heavy punch.
    High,
    /// Irreversible or security-critical operations — a knockout blow.
    /// Always requires explicit approval, even if auto-approve is enabled.
    Critical,
}

impl std::fmt::Display for RiskLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
            Self::Critical => write!(f, "critical"),
        }
    }
}

// ---------------------------------------------------------------------------
// Approval decision
// ---------------------------------------------------------------------------

/// The referee's decision on whether a move (tool call) is allowed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision", content = "reason")]
pub enum ApprovalDecision {
    /// The move is allowed — fight on.
    Allow,
    /// The move is denied — the fighter must stand down.
    Deny(String),
    /// The move requires human approval from a cornerman before proceeding.
    NeedsApproval(String),
}

// ---------------------------------------------------------------------------
// Approval request
// ---------------------------------------------------------------------------

/// A request submitted to the referee for a ruling on a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// The name of the tool (move) being invoked.
    pub tool_name: String,
    /// A human-readable summary of the tool's input parameters.
    pub input_summary: String,
    /// The assessed risk level of this action.
    pub risk_level: RiskLevel,
    /// The fighter attempting the move.
    pub fighter_id: FighterId,
    /// Why this request was flagged for review.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Approval policy
// ---------------------------------------------------------------------------

/// A policy rule that maps tool name patterns to risk levels and auto-approve
/// behavior.
///
/// Policies are the ring rules: they determine how much scrutiny each type of
/// move receives before a fighter is allowed to throw it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalPolicy {
    /// Human-readable name for this policy rule.
    pub name: String,
    /// Glob patterns matching tool names this policy applies to.
    pub tool_patterns: Vec<String>,
    /// Risk level assigned to matching tools.
    pub risk_level: RiskLevel,
    /// Whether matching tools are auto-approved (bypassing the handler).
    /// Note: Critical risk level always requires approval regardless of this flag.
    pub auto_approve: bool,
    /// Maximum number of auto-approvals before requiring manual approval.
    /// `None` means unlimited auto-approvals (as long as `auto_approve` is true).
    pub max_auto_approvals: Option<u32>,
}

// ---------------------------------------------------------------------------
// Approval handler trait
// ---------------------------------------------------------------------------

/// Trait for handling approval requests that require human (cornerman) input.
///
/// Implementations might prompt a CLI user, send a Slack message, call an
/// external webhook, or simply auto-approve/deny for testing and headless
/// operation.
#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    /// Request approval for a tool call. The cornerman reviews the request
    /// and returns their decision.
    async fn request_approval(&self, request: &ApprovalRequest) -> PunchResult<ApprovalDecision>;
}

// ---------------------------------------------------------------------------
// Built-in handlers
// ---------------------------------------------------------------------------

/// A handler that auto-approves every request — for headless/dev mode.
///
/// Like a ref who lets everything slide. Useful during development and
/// testing, but not recommended for production bouts.
#[derive(Debug, Clone)]
pub struct AutoApproveHandler;

#[async_trait]
impl ApprovalHandler for AutoApproveHandler {
    async fn request_approval(&self, _request: &ApprovalRequest) -> PunchResult<ApprovalDecision> {
        Ok(ApprovalDecision::Allow)
    }
}

/// A handler that denies every request — for locked-down mode.
///
/// The strictest ref in the business. No moves get through without an
/// explicit policy allowing them.
#[derive(Debug, Clone)]
pub struct DenyAllHandler;

#[async_trait]
impl ApprovalHandler for DenyAllHandler {
    async fn request_approval(&self, request: &ApprovalRequest) -> PunchResult<ApprovalDecision> {
        Ok(ApprovalDecision::Deny(format!(
            "all tool calls denied by policy: {}",
            request.tool_name
        )))
    }
}

// ---------------------------------------------------------------------------
// Policy engine
// ---------------------------------------------------------------------------

/// The referee engine that evaluates tool calls against configured policies.
///
/// The `PolicyEngine` holds a set of [`ApprovalPolicy`] rules and an
/// [`ApprovalHandler`] for escalating decisions that require human input.
/// It tracks auto-approval counts per policy to enforce rate limits.
pub struct PolicyEngine {
    /// The configured policy rules, evaluated in order (first match wins).
    policies: Vec<ApprovalPolicy>,
    /// The handler to call when a tool call requires human approval.
    handler: Arc<dyn ApprovalHandler>,
    /// Per-policy auto-approval counters, indexed by policy position.
    /// Uses `AtomicU32` for lock-free concurrent access.
    auto_approve_counts: Vec<AtomicU32>,
}

impl std::fmt::Debug for PolicyEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyEngine")
            .field("policies", &self.policies)
            .field(
                "auto_approve_counts",
                &self
                    .auto_approve_counts
                    .iter()
                    .map(|c| c.load(Ordering::Relaxed))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl PolicyEngine {
    /// Create a new policy engine with the given rules and handler.
    pub fn new(policies: Vec<ApprovalPolicy>, handler: Arc<dyn ApprovalHandler>) -> Self {
        let auto_approve_counts = policies.iter().map(|_| AtomicU32::new(0)).collect();
        Self {
            policies,
            handler,
            auto_approve_counts,
        }
    }

    /// Evaluate a tool call against the configured policies.
    ///
    /// The referee checks the ring rules:
    /// 1. Find the first policy whose tool patterns match the tool name
    /// 2. If no policy matches, the move is allowed (permissive by default)
    /// 3. If the matched policy auto-approves and the risk is not Critical,
    ///    check the auto-approval counter
    /// 4. If the counter is exhausted (or risk is Critical), escalate to the handler
    pub async fn evaluate(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
        fighter_id: &FighterId,
    ) -> PunchResult<ApprovalDecision> {
        // Find the first matching policy.
        let matched = self.find_matching_policy(tool_name);

        let Some((policy_index, policy)) = matched else {
            // No policy matched — permissive default, the move is allowed.
            return Ok(ApprovalDecision::Allow);
        };

        // Critical risk always requires approval, regardless of auto_approve flag.
        if policy.risk_level == RiskLevel::Critical {
            let request = Self::build_request(
                tool_name,
                input,
                policy.risk_level,
                fighter_id,
                &format!(
                    "critical risk tool '{}' matched policy '{}'",
                    tool_name, policy.name
                ),
            );
            return self.handler.request_approval(&request).await;
        }

        // Check auto-approve.
        if policy.auto_approve {
            if let Some(max) = policy.max_auto_approvals {
                let current =
                    self.auto_approve_counts[policy_index].fetch_add(1, Ordering::Relaxed);
                if current < max {
                    return Ok(ApprovalDecision::Allow);
                }
                // Counter exhausted — fall through to handler.
                let request = Self::build_request(
                    tool_name,
                    input,
                    policy.risk_level,
                    fighter_id,
                    &format!(
                        "auto-approval limit ({}) reached for policy '{}'",
                        max, policy.name
                    ),
                );
                return self.handler.request_approval(&request).await;
            }
            // Unlimited auto-approve.
            return Ok(ApprovalDecision::Allow);
        }

        // Policy matched but auto_approve is false — escalate to handler.
        let request = Self::build_request(
            tool_name,
            input,
            policy.risk_level,
            fighter_id,
            &format!(
                "tool '{}' matched policy '{}' (risk: {})",
                tool_name, policy.name, policy.risk_level
            ),
        );
        self.handler.request_approval(&request).await
    }

    /// Find the first policy whose tool patterns match the given tool name.
    /// Returns the policy index and a reference to the policy.
    fn find_matching_policy(&self, tool_name: &str) -> Option<(usize, &ApprovalPolicy)> {
        for (i, policy) in self.policies.iter().enumerate() {
            for pattern_str in &policy.tool_patterns {
                if pattern_str == "*" || pattern_str == "**" {
                    return Some((i, policy));
                }
                if let Ok(pattern) = glob::Pattern::new(pattern_str)
                    && pattern.matches(tool_name)
                {
                    return Some((i, policy));
                }
            }
        }
        None
    }

    /// Build an approval request with a summary of the tool input.
    fn build_request(
        tool_name: &str,
        input: &serde_json::Value,
        risk_level: RiskLevel,
        fighter_id: &FighterId,
        reason: &str,
    ) -> ApprovalRequest {
        // Build a concise summary of the input for human review.
        let input_summary = match input {
            serde_json::Value::Object(map) => {
                let pairs: Vec<String> = map
                    .iter()
                    .take(5)
                    .map(|(k, v)| {
                        let v_str = match v {
                            serde_json::Value::String(s) => {
                                if s.len() > 100 {
                                    format!("{}...", &s[..100])
                                } else {
                                    s.clone()
                                }
                            }
                            other => {
                                let s = other.to_string();
                                if s.len() > 100 {
                                    format!("{}...", &s[..100])
                                } else {
                                    s
                                }
                            }
                        };
                        format!("{}: {}", k, v_str)
                    })
                    .collect();
                pairs.join(", ")
            }
            other => {
                let s = other.to_string();
                if s.len() > 200 {
                    format!("{}...", &s[..200])
                } else {
                    s
                }
            }
        };

        ApprovalRequest {
            tool_name: tool_name.to_string(),
            input_summary,
            risk_level,
            fighter_id: *fighter_id,
            reason: reason.to_string(),
        }
    }

    /// Get the current auto-approval count for a policy at the given index.
    /// Returns `None` if the index is out of bounds.
    pub fn auto_approve_count(&self, policy_index: usize) -> Option<u32> {
        self.auto_approve_counts
            .get(policy_index)
            .map(|c| c.load(Ordering::Relaxed))
    }

    /// Reset all auto-approval counters to zero.
    pub fn reset_counters(&self) {
        for counter in &self.auto_approve_counts {
            counter.store(0, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn test_fighter_id() -> FighterId {
        FighterId(Uuid::nil())
    }

    // -- RiskLevel ordering --

    #[test]
    fn test_risk_level_ordering() {
        assert!(RiskLevel::Low < RiskLevel::Medium);
        assert!(RiskLevel::Medium < RiskLevel::High);
        assert!(RiskLevel::High < RiskLevel::Critical);
        assert!(RiskLevel::Low < RiskLevel::Critical);
    }

    // -- Policy matching with glob patterns --

    #[test]
    fn test_policy_matching_exact() {
        let engine = PolicyEngine::new(
            vec![ApprovalPolicy {
                name: "block-shell".into(),
                tool_patterns: vec!["shell_exec".into()],
                risk_level: RiskLevel::High,
                auto_approve: false,
                max_auto_approvals: None,
            }],
            Arc::new(DenyAllHandler),
        );
        let matched = engine.find_matching_policy("shell_exec");
        assert!(matched.is_some());
        assert_eq!(
            matched.as_ref().map(|(_, p)| p.name.as_str()),
            Some("block-shell")
        );
    }

    #[test]
    fn test_policy_matching_wildcard() {
        let engine = PolicyEngine::new(
            vec![ApprovalPolicy {
                name: "all-file-ops".into(),
                tool_patterns: vec!["file_*".into()],
                risk_level: RiskLevel::Medium,
                auto_approve: true,
                max_auto_approvals: None,
            }],
            Arc::new(AutoApproveHandler),
        );
        assert!(engine.find_matching_policy("file_read").is_some());
        assert!(engine.find_matching_policy("file_write").is_some());
        assert!(engine.find_matching_policy("file_list").is_some());
        assert!(engine.find_matching_policy("shell_exec").is_none());
    }

    #[test]
    fn test_policy_matching_no_match() {
        let engine = PolicyEngine::new(
            vec![ApprovalPolicy {
                name: "shell-only".into(),
                tool_patterns: vec!["shell_*".into()],
                risk_level: RiskLevel::High,
                auto_approve: false,
                max_auto_approvals: None,
            }],
            Arc::new(DenyAllHandler),
        );
        assert!(engine.find_matching_policy("file_read").is_none());
        assert!(engine.find_matching_policy("web_fetch").is_none());
    }

    // -- Auto-approve counter --

    #[tokio::test]
    async fn test_auto_approve_counter() {
        let engine = PolicyEngine::new(
            vec![ApprovalPolicy {
                name: "limited-reads".into(),
                tool_patterns: vec!["file_read".into()],
                risk_level: RiskLevel::Low,
                auto_approve: true,
                max_auto_approvals: Some(3),
            }],
            Arc::new(DenyAllHandler),
        );

        let fid = test_fighter_id();
        let input = serde_json::json!({"path": "test.txt"});

        // First 3 calls should be auto-approved.
        for _ in 0..3 {
            let decision = engine
                .evaluate("file_read", &input, &fid)
                .await
                .expect("evaluate failed");
            assert_eq!(decision, ApprovalDecision::Allow);
        }

        // 4th call should be denied (handler is DenyAllHandler).
        let decision = engine
            .evaluate("file_read", &input, &fid)
            .await
            .expect("evaluate failed");
        match decision {
            ApprovalDecision::Deny(_) => {} // expected
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    // -- AutoApproveHandler --

    #[tokio::test]
    async fn test_auto_approve_handler_always_approves() {
        let handler = AutoApproveHandler;
        let request = ApprovalRequest {
            tool_name: "shell_exec".into(),
            input_summary: "rm -rf /".into(),
            risk_level: RiskLevel::Critical,
            fighter_id: test_fighter_id(),
            reason: "test".into(),
        };
        let decision = handler
            .request_approval(&request)
            .await
            .expect("handler failed");
        assert_eq!(decision, ApprovalDecision::Allow);
    }

    // -- DenyAllHandler --

    #[tokio::test]
    async fn test_deny_all_handler_always_denies() {
        let handler = DenyAllHandler;
        let request = ApprovalRequest {
            tool_name: "file_read".into(),
            input_summary: "path: readme.md".into(),
            risk_level: RiskLevel::Low,
            fighter_id: test_fighter_id(),
            reason: "test".into(),
        };
        let decision = handler
            .request_approval(&request)
            .await
            .expect("handler failed");
        match decision {
            ApprovalDecision::Deny(_) => {} // expected
            other => panic!("expected Deny, got {:?}", other),
        }
    }

    // -- PolicyEngine::evaluate with multiple policies (first match wins) --

    #[tokio::test]
    async fn test_evaluate_first_match_wins() {
        let engine = PolicyEngine::new(
            vec![
                ApprovalPolicy {
                    name: "allow-file-read".into(),
                    tool_patterns: vec!["file_read".into()],
                    risk_level: RiskLevel::Low,
                    auto_approve: true,
                    max_auto_approvals: None,
                },
                ApprovalPolicy {
                    name: "deny-all-files".into(),
                    tool_patterns: vec!["file_*".into()],
                    risk_level: RiskLevel::High,
                    auto_approve: false,
                    max_auto_approvals: None,
                },
            ],
            Arc::new(DenyAllHandler),
        );

        let fid = test_fighter_id();
        let input = serde_json::json!({"path": "test.txt"});

        // file_read should match the first policy (auto-approve).
        let decision = engine
            .evaluate("file_read", &input, &fid)
            .await
            .expect("evaluate failed");
        assert_eq!(decision, ApprovalDecision::Allow);

        // file_write should match the second policy (deny).
        let decision = engine
            .evaluate("file_write", &input, &fid)
            .await
            .expect("evaluate failed");
        match decision {
            ApprovalDecision::Deny(_) => {} // expected
            other => panic!("expected Deny for file_write, got {:?}", other),
        }
    }

    // -- Empty policy list = allow all --

    #[tokio::test]
    async fn test_empty_policies_allow_all() {
        let engine = PolicyEngine::new(vec![], Arc::new(DenyAllHandler));
        let fid = test_fighter_id();
        let input = serde_json::json!({"command": "rm -rf /"});

        let decision = engine
            .evaluate("shell_exec", &input, &fid)
            .await
            .expect("evaluate failed");
        assert_eq!(decision, ApprovalDecision::Allow);
    }

    // -- Critical risk requires approval even with auto-approve --

    #[tokio::test]
    async fn test_critical_risk_requires_approval_even_with_auto_approve() {
        let engine = PolicyEngine::new(
            vec![ApprovalPolicy {
                name: "critical-shell".into(),
                tool_patterns: vec!["shell_exec".into()],
                risk_level: RiskLevel::Critical,
                auto_approve: true, // This should be ignored for Critical.
                max_auto_approvals: None,
            }],
            Arc::new(DenyAllHandler),
        );

        let fid = test_fighter_id();
        let input = serde_json::json!({"command": "rm -rf /"});

        // Even though auto_approve is true, critical risk should escalate.
        let decision = engine
            .evaluate("shell_exec", &input, &fid)
            .await
            .expect("evaluate failed");
        match decision {
            ApprovalDecision::Deny(_) => {} // DenyAllHandler denies it
            other => panic!("expected Deny for critical tool, got {:?}", other),
        }
    }

    // -- ApprovalRequest serialization --

    #[test]
    fn test_approval_request_serialization() {
        let request = ApprovalRequest {
            tool_name: "file_write".into(),
            input_summary: "path: /etc/passwd, content: hacked".into(),
            risk_level: RiskLevel::Critical,
            fighter_id: test_fighter_id(),
            reason: "critical operation detected".into(),
        };

        let json = serde_json::to_string(&request).expect("serialization failed");
        let deserialized: ApprovalRequest =
            serde_json::from_str(&json).expect("deserialization failed");

        assert_eq!(deserialized.tool_name, "file_write");
        assert_eq!(deserialized.risk_level, RiskLevel::Critical);
        assert_eq!(deserialized.reason, "critical operation detected");
    }

    // -- ApprovalDecision serialization --

    #[test]
    fn test_approval_decision_serialization() {
        let allow = ApprovalDecision::Allow;
        let deny = ApprovalDecision::Deny("not permitted".into());
        let needs = ApprovalDecision::NeedsApproval("requires human review".into());

        let allow_json = serde_json::to_string(&allow).expect("serialize allow");
        let deny_json = serde_json::to_string(&deny).expect("serialize deny");
        let needs_json = serde_json::to_string(&needs).expect("serialize needs_approval");

        let allow_back: ApprovalDecision = serde_json::from_str(&allow_json).expect("deser allow");
        let deny_back: ApprovalDecision = serde_json::from_str(&deny_json).expect("deser deny");
        let needs_back: ApprovalDecision = serde_json::from_str(&needs_json).expect("deser needs");

        assert_eq!(allow_back, ApprovalDecision::Allow);
        assert_eq!(deny_back, ApprovalDecision::Deny("not permitted".into()));
        assert_eq!(
            needs_back,
            ApprovalDecision::NeedsApproval("requires human review".into())
        );
    }

    // -- Wildcard catch-all policy --

    #[tokio::test]
    async fn test_catch_all_policy() {
        let engine = PolicyEngine::new(
            vec![ApprovalPolicy {
                name: "catch-all".into(),
                tool_patterns: vec!["*".into()],
                risk_level: RiskLevel::Medium,
                auto_approve: false,
                max_auto_approvals: None,
            }],
            Arc::new(DenyAllHandler),
        );

        let fid = test_fighter_id();
        let input = serde_json::json!({});

        // Every tool should match the catch-all and be denied.
        for tool in &["file_read", "shell_exec", "web_fetch", "memory_store"] {
            let decision = engine
                .evaluate(tool, &input, &fid)
                .await
                .expect("evaluate failed");
            match decision {
                ApprovalDecision::Deny(_) => {} // expected
                other => panic!("expected Deny for {}, got {:?}", tool, other),
            }
        }
    }

    // -- Reset counters --

    #[tokio::test]
    async fn test_reset_counters() {
        let engine = PolicyEngine::new(
            vec![ApprovalPolicy {
                name: "limited".into(),
                tool_patterns: vec!["file_read".into()],
                risk_level: RiskLevel::Low,
                auto_approve: true,
                max_auto_approvals: Some(2),
            }],
            Arc::new(DenyAllHandler),
        );

        let fid = test_fighter_id();
        let input = serde_json::json!({"path": "test.txt"});

        // Use up both auto-approvals.
        engine
            .evaluate("file_read", &input, &fid)
            .await
            .expect("eval 1");
        engine
            .evaluate("file_read", &input, &fid)
            .await
            .expect("eval 2");
        assert_eq!(engine.auto_approve_count(0), Some(2));

        // Reset and verify counter is back to zero.
        engine.reset_counters();
        assert_eq!(engine.auto_approve_count(0), Some(0));

        // Should auto-approve again.
        let decision = engine
            .evaluate("file_read", &input, &fid)
            .await
            .expect("eval after reset");
        assert_eq!(decision, ApprovalDecision::Allow);
    }
}
