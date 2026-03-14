//! Loop constructs for workflow steps.
//!
//! Supports `ForEach` (iterate over JSON arrays), `While` (repeat while
//! condition holds), and `Retry` (retry with backoff).

use serde::{Deserialize, Serialize};

use crate::workflow_conditions::Condition;

/// A loop construct that can be attached to a workflow step.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LoopConfig {
    /// Iterate over items from a JSON array in a previous step's output.
    ForEach {
        /// Name of the step whose output is a JSON array.
        source_step: String,
        /// Maximum number of iterations (safety limit).
        max_iterations: usize,
    },
    /// Repeat while a condition is true.
    While {
        /// The condition to evaluate each iteration.
        condition: Condition,
        /// Maximum iterations (safety limit, prevents infinite loops).
        max_iterations: usize,
    },
    /// Retry a step N times with configurable backoff.
    Retry {
        /// Maximum number of retry attempts.
        max_retries: usize,
        /// Initial backoff in milliseconds.
        backoff_ms: u64,
        /// Backoff multiplier (e.g. 2.0 for exponential backoff).
        backoff_multiplier: f64,
    },
}

/// Tracks the state of a loop during execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopState {
    /// Current iteration index (0-based).
    pub index: usize,
    /// Current item (for ForEach loops — the JSON string of the current element).
    pub item: Option<String>,
    /// Accumulated results from each iteration.
    pub accumulated_results: Vec<String>,
    /// Whether a break was requested.
    pub should_break: bool,
    /// Whether the current iteration should be skipped (continue).
    pub should_continue: bool,
}

impl LoopState {
    /// Create a new loop state.
    pub fn new() -> Self {
        Self {
            index: 0,
            item: None,
            accumulated_results: Vec::new(),
            should_break: false,
            should_continue: false,
        }
    }

    /// Request a break out of the loop.
    pub fn request_break(&mut self) {
        self.should_break = true;
    }

    /// Request a continue (skip rest of current iteration).
    pub fn request_continue(&mut self) {
        self.should_continue = true;
    }

    /// Advance to the next iteration, clearing per-iteration flags.
    pub fn advance(&mut self) {
        self.index += 1;
        self.should_continue = false;
    }

    /// Record the result of the current iteration.
    pub fn push_result(&mut self, result: String) {
        self.accumulated_results.push(result);
    }
}

impl Default for LoopState {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a JSON array string into a list of individual JSON value strings.
///
/// Returns an error string if parsing fails.
pub fn parse_foreach_items(json_str: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("failed to parse JSON array: {e}"))?;

    match value {
        serde_json::Value::Array(arr) => Ok(arr
            .into_iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            })
            .collect()),
        _ => Err("expected a JSON array".to_string()),
    }
}

/// Calculate the backoff duration for a given retry attempt.
pub fn calculate_backoff(attempt: usize, base_ms: u64, multiplier: f64) -> u64 {
    let factor = multiplier.powi(attempt as i32);
    (base_ms as f64 * factor) as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_state_new() {
        let state = LoopState::new();
        assert_eq!(state.index, 0);
        assert!(state.item.is_none());
        assert!(state.accumulated_results.is_empty());
        assert!(!state.should_break);
        assert!(!state.should_continue);
    }

    #[test]
    fn loop_state_default() {
        let state = LoopState::default();
        assert_eq!(state.index, 0);
    }

    #[test]
    fn loop_state_advance() {
        let mut state = LoopState::new();
        state.should_continue = true;
        state.advance();
        assert_eq!(state.index, 1);
        assert!(!state.should_continue);
    }

    #[test]
    fn loop_state_break() {
        let mut state = LoopState::new();
        state.request_break();
        assert!(state.should_break);
    }

    #[test]
    fn loop_state_continue() {
        let mut state = LoopState::new();
        state.request_continue();
        assert!(state.should_continue);
    }

    #[test]
    fn loop_state_push_result() {
        let mut state = LoopState::new();
        state.push_result("result1".to_string());
        state.push_result("result2".to_string());
        assert_eq!(state.accumulated_results.len(), 2);
        assert_eq!(state.accumulated_results[0], "result1");
        assert_eq!(state.accumulated_results[1], "result2");
    }

    #[test]
    fn parse_foreach_items_string_array() {
        let items = parse_foreach_items(r#"["a", "b", "c"]"#).expect("should parse");
        assert_eq!(items, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_foreach_items_number_array() {
        let items = parse_foreach_items("[1, 2, 3]").expect("should parse");
        assert_eq!(items, vec!["1", "2", "3"]);
    }

    #[test]
    fn parse_foreach_items_object_array() {
        let items = parse_foreach_items(r#"[{"name": "a"}, {"name": "b"}]"#).expect("should parse");
        assert_eq!(items.len(), 2);
        assert!(items[0].contains("name"));
    }

    #[test]
    fn parse_foreach_items_empty_array() {
        let items = parse_foreach_items("[]").expect("should parse");
        assert!(items.is_empty());
    }

    #[test]
    fn parse_foreach_items_not_array() {
        let result = parse_foreach_items(r#"{"key": "value"}"#);
        assert!(result.is_err());
        assert!(result.expect_err("error").contains("expected a JSON array"));
    }

    #[test]
    fn parse_foreach_items_invalid_json() {
        let result = parse_foreach_items("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn calculate_backoff_first_attempt() {
        let ms = calculate_backoff(0, 100, 2.0);
        assert_eq!(ms, 100);
    }

    #[test]
    fn calculate_backoff_exponential() {
        assert_eq!(calculate_backoff(1, 100, 2.0), 200);
        assert_eq!(calculate_backoff(2, 100, 2.0), 400);
        assert_eq!(calculate_backoff(3, 100, 2.0), 800);
    }

    #[test]
    fn calculate_backoff_no_multiplier() {
        assert_eq!(calculate_backoff(0, 500, 1.0), 500);
        assert_eq!(calculate_backoff(1, 500, 1.0), 500);
        assert_eq!(calculate_backoff(5, 500, 1.0), 500);
    }

    #[test]
    fn loop_config_foreach_serialization() {
        let config = LoopConfig::ForEach {
            source_step: "step1".to_string(),
            max_iterations: 100,
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let deser: LoopConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(config, deser);
    }

    #[test]
    fn loop_config_while_serialization() {
        let config = LoopConfig::While {
            condition: Condition::Always,
            max_iterations: 10,
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let deser: LoopConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(config, deser);
    }

    #[test]
    fn loop_config_retry_serialization() {
        let config = LoopConfig::Retry {
            max_retries: 3,
            backoff_ms: 100,
            backoff_multiplier: 2.0,
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let deser: LoopConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(config, deser);
    }

    #[test]
    fn loop_state_serialization() {
        let mut state = LoopState::new();
        state.index = 5;
        state.item = Some("test_item".to_string());
        state.push_result("r1".to_string());
        let json = serde_json::to_string(&state).expect("serialize");
        let deser: LoopState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.index, 5);
        assert_eq!(deser.item.as_deref(), Some("test_item"));
        assert_eq!(deser.accumulated_results.len(), 1);
    }

    #[test]
    fn parse_foreach_items_mixed_types() {
        let items = parse_foreach_items(r#"["hello", 42, true, null]"#).expect("should parse");
        assert_eq!(items.len(), 4);
        assert_eq!(items[0], "hello");
        assert_eq!(items[1], "42");
        assert_eq!(items[2], "true");
        assert_eq!(items[3], "null");
    }
}
