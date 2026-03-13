//! Loop guard / circuit breaker for the fighter loop.
//!
//! Detects repetitive tool call patterns, ping-pong cycles, and enforces
//! maximum iteration limits to prevent runaway agent loops.
//!
//! ## Graduated Response
//!
//! The guard uses a graduated response model:
//! - **Allow**: Normal operation
//! - **Warn**: Log a warning but allow the call (threshold: 3 identical calls)
//! - **Block**: Block this specific call (threshold: 5 identical calls)
//! - **CircuitBreak**: Terminate the entire loop (threshold: 30 total iterations)

use std::collections::{HashMap, HashSet, VecDeque};

use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use punch_types::ToolCall;

/// Default thresholds for the graduated response.
const DEFAULT_WARN_THRESHOLD: usize = 3;
const DEFAULT_BLOCK_THRESHOLD: usize = 5;
const DEFAULT_CIRCUIT_BREAKER_THRESHOLD: usize = 30;

/// Size of the recent calls ring buffer for pattern detection.
const RECENT_CALLS_BUFFER_SIZE: usize = 30;

/// Multiplier for "poll" tools (e.g. shell_exec) that naturally repeat.
const POLL_TOOL_THRESHOLD_MULTIPLIER: usize = 3;

/// Poll tool names that get relaxed thresholds.
const POLL_TOOLS: &[&str] = &["shell_exec"];

/// Graduated response level from the guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardVerdict {
    /// Normal operation, proceed.
    Allow,
    /// Allowed but suspicious — a warning has been logged.
    Warn(String),
    /// This specific call is blocked. The message explains why.
    /// The loop should skip this tool call and insert an error result.
    Block(String),
    /// The entire loop must terminate. The message explains why.
    CircuitBreak(String),
}

impl GuardVerdict {
    /// Whether the verdict allows continuing (Allow or Warn).
    pub fn is_allowed(&self) -> bool {
        matches!(self, GuardVerdict::Allow | GuardVerdict::Warn(_))
    }

    /// Whether the verdict requires terminating the loop.
    pub fn is_circuit_break(&self) -> bool {
        matches!(self, GuardVerdict::CircuitBreak(_))
    }
}

/// Configuration for the loop guard thresholds.
#[derive(Debug, Clone)]
pub struct GuardConfig {
    /// Max iterations before circuit break.
    pub max_iterations: usize,
    /// Number of identical calls before warning.
    pub warn_threshold: usize,
    /// Number of identical calls before blocking.
    pub block_threshold: usize,
    /// Total iterations before circuit break (separate from max_iterations).
    pub circuit_breaker_threshold: usize,
}

impl Default for GuardConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            warn_threshold: DEFAULT_WARN_THRESHOLD,
            block_threshold: DEFAULT_BLOCK_THRESHOLD,
            circuit_breaker_threshold: DEFAULT_CIRCUIT_BREAKER_THRESHOLD,
        }
    }
}

/// Circuit breaker that watches for repetitive tool call patterns and
/// enforces a maximum iteration count.
///
/// Tracks call fingerprints, outcome fingerprints, and recent call history
/// to detect various pathological patterns including ping-pong cycles.
#[derive(Debug)]
pub struct LoopGuard {
    config: GuardConfig,
    /// Current iteration count.
    current_iteration: usize,
    /// Counts of each observed call fingerprint (SHA-256 of tool_name + params).
    call_counts: HashMap<String, usize>,
    /// Counts of each observed outcome fingerprint (SHA-256 of call + result).
    outcome_counts: HashMap<String, usize>,
    /// Set of outcome hashes that have been auto-blocked.
    blocked_outcomes: HashSet<String>,
    /// Ring buffer of recent call hashes for pattern detection.
    recent_calls: VecDeque<String>,
}

impl LoopGuard {
    /// Create a new loop guard with default configuration.
    pub fn new(max_iterations: usize, _repetition_threshold: usize) -> Self {
        Self::with_config(GuardConfig {
            max_iterations,
            ..Default::default()
        })
    }

    /// Create a new loop guard with explicit configuration.
    pub fn with_config(config: GuardConfig) -> Self {
        Self {
            config,
            current_iteration: 0,
            call_counts: HashMap::new(),
            outcome_counts: HashMap::new(),
            blocked_outcomes: HashSet::new(),
            recent_calls: VecDeque::with_capacity(RECENT_CALLS_BUFFER_SIZE),
        }
    }

    /// Record a set of tool calls and return a verdict.
    ///
    /// This is the main entry point for the guard. Call this before
    /// executing each batch of tool calls from the LLM.
    pub fn record_tool_calls(&mut self, tool_calls: &[ToolCall]) -> LoopGuardVerdict {
        self.current_iteration += 1;

        // Check absolute iteration limit.
        if self.current_iteration >= self.config.max_iterations {
            return LoopGuardVerdict::Break(format!(
                "maximum iterations reached ({}/{})",
                self.current_iteration, self.config.max_iterations
            ));
        }

        // Check circuit breaker threshold.
        if self.current_iteration >= self.config.circuit_breaker_threshold {
            return LoopGuardVerdict::Break(format!(
                "circuit breaker triggered after {} iterations",
                self.current_iteration
            ));
        }

        // Check each individual tool call.
        for tc in tool_calls {
            let call_hash = hash_call(tc);

            // Track in recent calls ring buffer.
            if self.recent_calls.len() >= RECENT_CALLS_BUFFER_SIZE {
                self.recent_calls.pop_front();
            }
            self.recent_calls.push_back(call_hash.clone());

            // Determine effective thresholds (poll tools get relaxed limits).
            // Compute before mutable borrow of call_counts.
            let (warn_t, block_t) = self.effective_thresholds(&tc.name);

            // Increment call count.
            let count = self.call_counts.entry(call_hash.clone()).or_insert(0);
            *count += 1;
            let current_count = *count;

            if current_count >= block_t {
                return LoopGuardVerdict::Break(format!(
                    "tool '{}' blocked: {} identical calls (threshold: {})",
                    tc.name, current_count, block_t
                ));
            }

            if current_count >= warn_t {
                warn!(
                    tool = %tc.name,
                    count = current_count,
                    threshold = warn_t,
                    "repetitive tool call detected"
                );
            }
        }

        // Check for ping-pong pattern (A-B-A-B).
        if let Some(reason) = self.detect_ping_pong() {
            return LoopGuardVerdict::Break(reason);
        }

        LoopGuardVerdict::Continue
    }

    /// Evaluate a single tool call and return a graduated verdict.
    ///
    /// Use this for finer-grained control where you want to handle
    /// Block vs CircuitBreak differently.
    pub fn evaluate_call(&mut self, tool_call: &ToolCall) -> GuardVerdict {
        let call_hash = hash_call(tool_call);

        // Track in recent calls ring buffer.
        if self.recent_calls.len() >= RECENT_CALLS_BUFFER_SIZE {
            self.recent_calls.pop_front();
        }
        self.recent_calls.push_back(call_hash.clone());

        // Compute thresholds before mutable borrow of call_counts.
        let (warn_t, block_t) = self.effective_thresholds(&tool_call.name);

        // Increment call count.
        let count = self.call_counts.entry(call_hash).or_insert(0);
        *count += 1;
        let current_count = *count;

        if current_count >= block_t {
            GuardVerdict::Block(format!(
                "tool '{}' blocked after {} identical calls",
                tool_call.name, current_count
            ))
        } else if current_count >= warn_t {
            GuardVerdict::Warn(format!(
                "tool '{}' called {} times with identical params (warn threshold: {})",
                tool_call.name, current_count, warn_t
            ))
        } else {
            GuardVerdict::Allow
        }
    }

    /// Record the outcome of a tool call (call hash + result hash).
    ///
    /// If the same outcome has been seen before, it is auto-blocked for
    /// future iterations.
    pub fn record_outcome(&mut self, tool_call: &ToolCall, result: &str) {
        let outcome_hash = hash_outcome(tool_call, result);
        let count = self.outcome_counts.entry(outcome_hash.clone()).or_insert(0);
        *count += 1;

        if *count >= 2 {
            debug!(
                tool = %tool_call.name,
                outcome_count = *count,
                "auto-blocking repeated identical outcome"
            );
            self.blocked_outcomes.insert(outcome_hash);
        }
    }

    /// Check if an outcome would be blocked (same call + same result seen before).
    pub fn is_outcome_blocked(&self, tool_call: &ToolCall, result: &str) -> bool {
        let outcome_hash = hash_outcome(tool_call, result);
        self.blocked_outcomes.contains(&outcome_hash)
    }

    /// Record a non-tool-call iteration (e.g. text-only response check).
    pub fn record_iteration(&mut self) -> LoopGuardVerdict {
        self.current_iteration += 1;

        if self.current_iteration >= self.config.max_iterations {
            return LoopGuardVerdict::Break(format!(
                "maximum iterations reached ({}/{})",
                self.current_iteration, self.config.max_iterations
            ));
        }

        LoopGuardVerdict::Continue
    }

    /// Current iteration count.
    pub fn iterations(&self) -> usize {
        self.current_iteration
    }

    /// Get the effective thresholds for a tool, accounting for poll tools.
    fn effective_thresholds(&self, tool_name: &str) -> (usize, usize) {
        if POLL_TOOLS.contains(&tool_name) {
            (
                self.config.warn_threshold * POLL_TOOL_THRESHOLD_MULTIPLIER,
                self.config.block_threshold * POLL_TOOL_THRESHOLD_MULTIPLIER,
            )
        } else {
            (self.config.warn_threshold, self.config.block_threshold)
        }
    }

    /// Detect A-B-A-B ping-pong patterns in recent calls.
    ///
    /// Looks for alternating patterns of length >= 4 (two full cycles).
    fn detect_ping_pong(&self) -> Option<String> {
        let len = self.recent_calls.len();
        if len < 4 {
            return None;
        }

        // Check the last 4+ calls for an A-B-A-B pattern.
        // We check from the end of the buffer.
        let calls: Vec<&String> = self.recent_calls.iter().collect();

        // Check for period-2 pattern (A-B-A-B) in the last 6 entries.
        let check_len = len.min(6);
        if check_len >= 4 {
            let tail = &calls[len - check_len..];
            let mut is_ping_pong = true;

            for i in 2..tail.len() {
                if tail[i] != tail[i - 2] {
                    is_ping_pong = false;
                    break;
                }
            }

            if is_ping_pong && tail.len() >= 4 && tail[0] != tail[1] {
                info!(
                    pattern_length = tail.len(),
                    "ping-pong pattern detected in tool calls"
                );
                return Some(format!(
                    "ping-pong pattern detected: alternating tool calls over {} iterations",
                    tail.len()
                ));
            }
        }

        None
    }
}

/// Legacy verdict type for backward compatibility with the fighter loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopGuardVerdict {
    /// The loop may continue.
    Continue,
    /// The loop must break for the given reason.
    Break(String),
}

/// Compute a SHA-256 fingerprint of a tool call (name + input).
fn hash_call(tool_call: &ToolCall) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tool_call.name.as_bytes());
    hasher.update(b"|");
    hasher.update(tool_call.input.to_string().as_bytes());
    let result = hasher.finalize();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, result)
}

/// Compute a SHA-256 fingerprint of a tool call outcome (call + result).
fn hash_outcome(tool_call: &ToolCall, result: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(tool_call.name.as_bytes());
    hasher.update(b"|");
    hasher.update(tool_call.input.to_string().as_bytes());
    hasher.update(b"|");
    hasher.update(result.as_bytes());
    let result_hash = hasher.finalize();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, result_hash)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use punch_types::ToolCall;

    fn make_tool_call(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: format!("call_{name}"),
            name: name.to_string(),
            input,
        }
    }

    #[test]
    fn test_max_iterations_enforcement() {
        let mut guard = LoopGuard::new(3, 5);

        assert_eq!(guard.record_iteration(), LoopGuardVerdict::Continue);
        assert_eq!(guard.record_iteration(), LoopGuardVerdict::Continue);

        match guard.record_iteration() {
            LoopGuardVerdict::Break(reason) => {
                assert!(reason.contains("maximum iterations"));
            }
            LoopGuardVerdict::Continue => panic!("should have broken"),
        }
    }

    #[test]
    fn test_repetitive_pattern_detection() {
        let mut guard = LoopGuard::with_config(GuardConfig {
            max_iterations: 50,
            warn_threshold: 3,
            block_threshold: 5,
            circuit_breaker_threshold: 30,
        });

        let calls = vec![make_tool_call(
            "file_read",
            serde_json::json!({"path": "/tmp/foo.txt"}),
        )];

        // Should get through warn threshold (3) without breaking.
        assert_eq!(guard.record_tool_calls(&calls), LoopGuardVerdict::Continue);
        assert_eq!(guard.record_tool_calls(&calls), LoopGuardVerdict::Continue);
        assert_eq!(guard.record_tool_calls(&calls), LoopGuardVerdict::Continue);
        assert_eq!(guard.record_tool_calls(&calls), LoopGuardVerdict::Continue);

        // 5th identical call should trigger block.
        match guard.record_tool_calls(&calls) {
            LoopGuardVerdict::Break(reason) => {
                assert!(reason.contains("blocked"));
            }
            LoopGuardVerdict::Continue => panic!("should have blocked"),
        }
    }

    #[test]
    fn test_different_calls_no_repetition() {
        let mut guard = LoopGuard::new(50, 3);

        let calls_a = vec![make_tool_call(
            "file_read",
            serde_json::json!({"path": "/a.txt"}),
        )];
        let calls_b = vec![make_tool_call(
            "file_read",
            serde_json::json!({"path": "/b.txt"}),
        )];
        let calls_c = vec![make_tool_call(
            "file_write",
            serde_json::json!({"path": "/c.txt", "content": "hi"}),
        )];

        assert_eq!(
            guard.record_tool_calls(&calls_a),
            LoopGuardVerdict::Continue
        );
        assert_eq!(
            guard.record_tool_calls(&calls_b),
            LoopGuardVerdict::Continue
        );
        assert_eq!(
            guard.record_tool_calls(&calls_c),
            LoopGuardVerdict::Continue
        );
    }

    #[test]
    fn test_iteration_counter() {
        let mut guard = LoopGuard::new(100, 3);

        let calls = vec![make_tool_call("test", serde_json::json!({}))];
        guard.record_tool_calls(&calls);
        guard.record_iteration();

        assert_eq!(guard.iterations(), 2);
    }

    #[test]
    fn test_graduated_response_warn() {
        let mut guard = LoopGuard::with_config(GuardConfig {
            max_iterations: 50,
            warn_threshold: 2,
            block_threshold: 5,
            circuit_breaker_threshold: 30,
        });

        let tc = make_tool_call("file_read", serde_json::json!({"path": "/test"}));

        assert_eq!(guard.evaluate_call(&tc), GuardVerdict::Allow);
        // Second call should warn.
        match guard.evaluate_call(&tc) {
            GuardVerdict::Warn(msg) => assert!(msg.contains("file_read")),
            other => panic!("expected Warn, got {:?}", other),
        }
    }

    #[test]
    fn test_graduated_response_block() {
        let mut guard = LoopGuard::with_config(GuardConfig {
            max_iterations: 50,
            warn_threshold: 2,
            block_threshold: 3,
            circuit_breaker_threshold: 30,
        });

        let tc = make_tool_call("file_read", serde_json::json!({"path": "/test"}));

        guard.evaluate_call(&tc); // 1 - Allow
        guard.evaluate_call(&tc); // 2 - Warn
        match guard.evaluate_call(&tc) {
            GuardVerdict::Block(msg) => assert!(msg.contains("blocked")),
            other => panic!("expected Block, got {:?}", other),
        }
    }

    #[test]
    fn test_poll_tool_relaxed_thresholds() {
        let mut guard = LoopGuard::with_config(GuardConfig {
            max_iterations: 50,
            warn_threshold: 3,
            block_threshold: 5,
            circuit_breaker_threshold: 50,
        });

        let tc = make_tool_call("shell_exec", serde_json::json!({"command": "ls"}));

        // shell_exec gets 3x thresholds: warn=9, block=15
        for _ in 0..8 {
            assert_eq!(guard.evaluate_call(&tc), GuardVerdict::Allow);
        }
        // 9th call should warn.
        match guard.evaluate_call(&tc) {
            GuardVerdict::Warn(_) => {}
            other => panic!("expected Warn at 9, got {:?}", other),
        }
    }

    #[test]
    fn test_outcome_tracking() {
        let mut guard = LoopGuard::new(50, 3);

        let tc = make_tool_call("file_read", serde_json::json!({"path": "/test"}));

        assert!(!guard.is_outcome_blocked(&tc, "file contents"));

        guard.record_outcome(&tc, "file contents");
        assert!(!guard.is_outcome_blocked(&tc, "file contents"));

        // Second identical outcome should trigger auto-block.
        guard.record_outcome(&tc, "file contents");
        assert!(guard.is_outcome_blocked(&tc, "file contents"));

        // Different result should not be blocked.
        assert!(!guard.is_outcome_blocked(&tc, "different contents"));
    }

    #[test]
    fn test_ping_pong_detection() {
        let mut guard = LoopGuard::with_config(GuardConfig {
            max_iterations: 50,
            warn_threshold: 10,
            block_threshold: 20,
            circuit_breaker_threshold: 50,
        });

        let call_a = vec![make_tool_call(
            "file_read",
            serde_json::json!({"path": "/a"}),
        )];
        let call_b = vec![make_tool_call(
            "file_read",
            serde_json::json!({"path": "/b"}),
        )];

        // A, B, A, B pattern
        assert_eq!(guard.record_tool_calls(&call_a), LoopGuardVerdict::Continue);
        assert_eq!(guard.record_tool_calls(&call_b), LoopGuardVerdict::Continue);
        assert_eq!(guard.record_tool_calls(&call_a), LoopGuardVerdict::Continue);

        // 4th call completes the A-B-A-B pattern.
        match guard.record_tool_calls(&call_b) {
            LoopGuardVerdict::Break(reason) => {
                assert!(reason.contains("ping-pong"));
            }
            LoopGuardVerdict::Continue => panic!("should have detected ping-pong"),
        }
    }

    #[test]
    fn test_circuit_breaker_threshold() {
        let mut guard = LoopGuard::with_config(GuardConfig {
            max_iterations: 100,
            warn_threshold: 50,
            block_threshold: 50,
            circuit_breaker_threshold: 5,
        });

        // Each iteration uses a different call to avoid repetition blocks.
        for i in 0..4 {
            let calls = vec![make_tool_call(
                "file_read",
                serde_json::json!({"path": format!("/file_{}", i)}),
            )];
            assert_eq!(guard.record_tool_calls(&calls), LoopGuardVerdict::Continue);
        }

        // 5th iteration should trigger circuit breaker.
        let calls = vec![make_tool_call(
            "file_read",
            serde_json::json!({"path": "/file_4"}),
        )];
        match guard.record_tool_calls(&calls) {
            LoopGuardVerdict::Break(reason) => {
                assert!(reason.contains("circuit breaker"));
            }
            LoopGuardVerdict::Continue => panic!("should have circuit broken"),
        }
    }

    #[test]
    fn test_guard_verdict_is_allowed() {
        assert!(GuardVerdict::Allow.is_allowed());
        assert!(GuardVerdict::Warn("test".into()).is_allowed());
        assert!(!GuardVerdict::Block("test".into()).is_allowed());
        assert!(!GuardVerdict::CircuitBreak("test".into()).is_allowed());
    }

    #[test]
    fn test_guard_verdict_is_circuit_break() {
        assert!(!GuardVerdict::Allow.is_circuit_break());
        assert!(!GuardVerdict::Warn("test".into()).is_circuit_break());
        assert!(!GuardVerdict::Block("test".into()).is_circuit_break());
        assert!(GuardVerdict::CircuitBreak("test".into()).is_circuit_break());
    }

    // -----------------------------------------------------------------------
    // Additional guard tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_guard_config_default() {
        let config = GuardConfig::default();
        assert_eq!(config.max_iterations, 50);
        assert_eq!(config.warn_threshold, 3);
        assert_eq!(config.block_threshold, 5);
        assert_eq!(config.circuit_breaker_threshold, 30);
    }

    #[test]
    fn test_loop_guard_new() {
        let guard = LoopGuard::new(10, 5);
        assert_eq!(guard.iterations(), 0);
    }

    #[test]
    fn test_loop_guard_iterations_incremented_by_tool_calls() {
        let mut guard = LoopGuard::new(50, 5);
        let calls = vec![make_tool_call("test", serde_json::json!({}))];
        guard.record_tool_calls(&calls);
        assert_eq!(guard.iterations(), 1);
    }

    #[test]
    fn test_loop_guard_iterations_incremented_by_record_iteration() {
        let mut guard = LoopGuard::new(50, 5);
        guard.record_iteration();
        guard.record_iteration();
        guard.record_iteration();
        assert_eq!(guard.iterations(), 3);
    }

    #[test]
    fn test_evaluate_call_first_call_is_allow() {
        let mut guard = LoopGuard::new(50, 5);
        let tc = make_tool_call("file_read", serde_json::json!({"path": "/tmp/test"}));
        assert_eq!(guard.evaluate_call(&tc), GuardVerdict::Allow);
    }

    #[test]
    fn test_evaluate_call_different_params_no_warn() {
        let mut guard = LoopGuard::with_config(GuardConfig {
            max_iterations: 50,
            warn_threshold: 2,
            block_threshold: 5,
            circuit_breaker_threshold: 50,
        });

        let tc1 = make_tool_call("file_read", serde_json::json!({"path": "/a"}));
        let tc2 = make_tool_call("file_read", serde_json::json!({"path": "/b"}));

        assert_eq!(guard.evaluate_call(&tc1), GuardVerdict::Allow);
        assert_eq!(guard.evaluate_call(&tc2), GuardVerdict::Allow);
    }

    #[test]
    fn test_hash_call_deterministic() {
        let tc = make_tool_call("test", serde_json::json!({"key": "value"}));
        let h1 = hash_call(&tc);
        let h2 = hash_call(&tc);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_call_different_for_different_inputs() {
        let tc1 = make_tool_call("test", serde_json::json!({"key": "value1"}));
        let tc2 = make_tool_call("test", serde_json::json!({"key": "value2"}));
        assert_ne!(hash_call(&tc1), hash_call(&tc2));
    }

    #[test]
    fn test_hash_outcome_deterministic() {
        let tc = make_tool_call("test", serde_json::json!({}));
        let h1 = hash_outcome(&tc, "result");
        let h2 = hash_outcome(&tc, "result");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_outcome_different_for_different_results() {
        let tc = make_tool_call("test", serde_json::json!({}));
        assert_ne!(hash_outcome(&tc, "result1"), hash_outcome(&tc, "result2"));
    }

    #[test]
    fn test_outcome_blocked_only_after_two() {
        let mut guard = LoopGuard::new(50, 5);
        let tc = make_tool_call("test", serde_json::json!({}));

        assert!(!guard.is_outcome_blocked(&tc, "result"));
        guard.record_outcome(&tc, "result");
        assert!(!guard.is_outcome_blocked(&tc, "result"));
        guard.record_outcome(&tc, "result");
        assert!(guard.is_outcome_blocked(&tc, "result"));
    }

    #[test]
    fn test_no_ping_pong_with_three_calls() {
        let mut guard = LoopGuard::with_config(GuardConfig {
            max_iterations: 50,
            warn_threshold: 10,
            block_threshold: 20,
            circuit_breaker_threshold: 50,
        });

        let call_a = vec![make_tool_call("a", serde_json::json!({}))];
        let call_b = vec![make_tool_call("b", serde_json::json!({}))];

        // Only A, B, A -- not enough for ping-pong (need 4)
        assert_eq!(guard.record_tool_calls(&call_a), LoopGuardVerdict::Continue);
        assert_eq!(guard.record_tool_calls(&call_b), LoopGuardVerdict::Continue);
        assert_eq!(guard.record_tool_calls(&call_a), LoopGuardVerdict::Continue);
    }

    #[test]
    fn test_no_ping_pong_same_call_repeated() {
        let mut guard = LoopGuard::with_config(GuardConfig {
            max_iterations: 50,
            warn_threshold: 10,
            block_threshold: 20,
            circuit_breaker_threshold: 50,
        });

        let call_a = vec![make_tool_call("a", serde_json::json!({}))];

        // A, A, A, A is not a ping-pong pattern (needs A != B)
        for _ in 0..4 {
            guard.record_tool_calls(&call_a);
        }
        // Should not break due to ping-pong (might warn for repetition)
    }

    #[test]
    fn test_loop_guard_verdict_continue_equality() {
        assert_eq!(LoopGuardVerdict::Continue, LoopGuardVerdict::Continue);
    }

    #[test]
    fn test_loop_guard_verdict_break_equality() {
        assert_eq!(
            LoopGuardVerdict::Break("reason".into()),
            LoopGuardVerdict::Break("reason".into())
        );
        assert_ne!(
            LoopGuardVerdict::Break("reason1".into()),
            LoopGuardVerdict::Break("reason2".into())
        );
    }

    #[test]
    fn test_effective_thresholds_normal_tool() {
        let guard = LoopGuard::with_config(GuardConfig {
            max_iterations: 50,
            warn_threshold: 3,
            block_threshold: 5,
            circuit_breaker_threshold: 30,
        });
        let (warn, block) = guard.effective_thresholds("file_read");
        assert_eq!(warn, 3);
        assert_eq!(block, 5);
    }

    #[test]
    fn test_effective_thresholds_poll_tool() {
        let guard = LoopGuard::with_config(GuardConfig {
            max_iterations: 50,
            warn_threshold: 3,
            block_threshold: 5,
            circuit_breaker_threshold: 30,
        });
        let (warn, block) = guard.effective_thresholds("shell_exec");
        assert_eq!(warn, 9); // 3 * 3
        assert_eq!(block, 15); // 5 * 3
    }
}
