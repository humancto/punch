//! Loop guard / circuit breaker for the fighter loop.
//!
//! Detects repetitive tool call patterns and enforces maximum iteration limits
//! to prevent runaway agent loops.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use punch_types::ToolCall;

/// Verdict from the loop guard after recording a cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopGuardVerdict {
    /// The loop may continue.
    Continue,
    /// The loop must break for the given reason.
    Break(String),
}

/// Circuit breaker that watches for repetitive tool call patterns and
/// enforces a maximum iteration count.
#[derive(Debug)]
pub struct LoopGuard {
    /// Maximum number of iterations before forced termination.
    max_iterations: usize,
    /// Current iteration count.
    current_iteration: usize,
    /// Number of times a hash must repeat to trigger a break.
    repetition_threshold: usize,
    /// Counts of each observed tool-call-sequence hash.
    hash_counts: HashMap<String, usize>,
}

impl LoopGuard {
    /// Create a new loop guard with the given limits.
    pub fn new(max_iterations: usize, repetition_threshold: usize) -> Self {
        Self {
            max_iterations,
            current_iteration: 0,
            repetition_threshold,
            hash_counts: HashMap::new(),
        }
    }

    /// Record a set of tool calls from one iteration and return a verdict.
    ///
    /// The tool calls are hashed by name + input to detect identical sequences.
    pub fn record_tool_calls(&mut self, tool_calls: &[ToolCall]) -> LoopGuardVerdict {
        self.current_iteration += 1;

        // Check max iterations.
        if self.current_iteration >= self.max_iterations {
            return LoopGuardVerdict::Break(format!(
                "maximum iterations reached ({}/{})",
                self.current_iteration, self.max_iterations
            ));
        }

        // Hash the tool call sequence.
        let hash = Self::hash_tool_calls(tool_calls);
        let count = self.hash_counts.entry(hash).or_insert(0);
        *count += 1;

        if *count >= self.repetition_threshold {
            return LoopGuardVerdict::Break(format!(
                "repetitive tool call pattern detected ({} identical cycles)",
                *count
            ));
        }

        LoopGuardVerdict::Continue
    }

    /// Record a non-tool-call iteration (e.g. text-only response check).
    pub fn record_iteration(&mut self) -> LoopGuardVerdict {
        self.current_iteration += 1;

        if self.current_iteration >= self.max_iterations {
            return LoopGuardVerdict::Break(format!(
                "maximum iterations reached ({}/{})",
                self.current_iteration, self.max_iterations
            ));
        }

        LoopGuardVerdict::Continue
    }

    /// Current iteration count.
    pub fn iterations(&self) -> usize {
        self.current_iteration
    }

    /// Compute a SHA-256 hash over an ordered sequence of tool calls.
    fn hash_tool_calls(tool_calls: &[ToolCall]) -> String {
        let mut hasher = Sha256::new();

        for tc in tool_calls {
            hasher.update(tc.name.as_bytes());
            hasher.update(b"|");
            hasher.update(tc.input.to_string().as_bytes());
            hasher.update(b";");
        }

        let result = hasher.finalize();
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, result)
    }
}

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
        let mut guard = LoopGuard::new(50, 3);

        let calls = vec![make_tool_call(
            "file_read",
            serde_json::json!({"path": "/tmp/foo.txt"}),
        )];

        assert_eq!(guard.record_tool_calls(&calls), LoopGuardVerdict::Continue);
        assert_eq!(guard.record_tool_calls(&calls), LoopGuardVerdict::Continue);

        match guard.record_tool_calls(&calls) {
            LoopGuardVerdict::Break(reason) => {
                assert!(reason.contains("repetitive"));
            }
            LoopGuardVerdict::Continue => panic!("should have detected repetition"),
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
}
