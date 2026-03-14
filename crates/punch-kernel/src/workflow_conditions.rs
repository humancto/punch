//! Conditional branching for workflow steps.
//!
//! Each workflow step may carry an optional [`Condition`] that determines
//! whether the step should execute based on results from prior steps.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::workflow::StepResult;

/// A condition that gates step execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    /// Always execute.
    Always,
    /// Execute only if the named step's output contains the given substring.
    IfOutput { step: String, contains: String },
    /// Execute only if the named step completed successfully.
    IfSuccess { step: String },
    /// Execute only if the named step failed.
    IfFailure { step: String },
    /// Simple expression evaluator (supports basic boolean logic).
    Expression(String),
}

/// Evaluate a [`Condition`] against the current set of completed step results.
///
/// Returns `true` if the step should execute, `false` if it should be skipped.
pub fn evaluate_condition(
    condition: &Condition,
    step_results: &HashMap<String, StepResult>,
) -> bool {
    match condition {
        Condition::Always => true,
        Condition::IfOutput { step, contains } => step_results
            .get(step)
            .map(|r| r.response.contains(contains.as_str()))
            .unwrap_or(false),
        Condition::IfSuccess { step } => step_results
            .get(step)
            .map(|r| r.error.is_none())
            .unwrap_or(false),
        Condition::IfFailure { step } => step_results
            .get(step)
            .map(|r| r.error.is_some())
            .unwrap_or(false),
        Condition::Expression(expr) => evaluate_expression(expr, step_results),
    }
}

/// Evaluate a simple boolean expression.
///
/// Supports:
/// - `step_name.success` — true if step succeeded
/// - `step_name.failed` — true if step failed
/// - `step_name.output contains "text"` — true if output contains text
/// - `not <expr>` — negation
/// - `<expr> and <expr>` — conjunction
/// - `<expr> or <expr>` — disjunction
/// - `true` / `false` — literals
fn evaluate_expression(expr: &str, step_results: &HashMap<String, StepResult>) -> bool {
    let expr = expr.trim();

    // Handle `true` / `false` literals
    if expr.eq_ignore_ascii_case("true") {
        return true;
    }
    if expr.eq_ignore_ascii_case("false") {
        return false;
    }

    // Handle `not` prefix
    if let Some(rest) = expr.strip_prefix("not ") {
        return !evaluate_expression(rest, step_results);
    }

    // Handle `and` (lowest precedence after `or`)
    // We split on ` or ` first (lower precedence)
    if let Some(pos) = expr.find(" or ") {
        let left = &expr[..pos];
        let right = &expr[pos + 4..];
        return evaluate_expression(left, step_results) || evaluate_expression(right, step_results);
    }

    // Then split on ` and `
    if let Some(pos) = expr.find(" and ") {
        let left = &expr[..pos];
        let right = &expr[pos + 5..];
        return evaluate_expression(left, step_results) && evaluate_expression(right, step_results);
    }

    // Handle `step_name.success`
    if let Some(step_name) = expr.strip_suffix(".success") {
        return step_results
            .get(step_name)
            .map(|r| r.error.is_none())
            .unwrap_or(false);
    }

    // Handle `step_name.failed`
    if let Some(step_name) = expr.strip_suffix(".failed") {
        return step_results
            .get(step_name)
            .map(|r| r.error.is_some())
            .unwrap_or(false);
    }

    // Handle `step_name.output contains "text"`
    if let Some(contains_pos) = expr.find(".output contains ") {
        let step_name = &expr[..contains_pos];
        let rest = &expr[contains_pos + ".output contains ".len()..];
        let text = rest.trim_matches('"');
        return step_results
            .get(step_name)
            .map(|r| r.response.contains(text))
            .unwrap_or(false);
    }

    // Unknown expression — default to false
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_results() -> HashMap<String, StepResult> {
        let mut results = HashMap::new();
        results.insert(
            "analyze".to_string(),
            StepResult {
                step_name: "analyze".to_string(),
                response: "The code has 3 bugs and needs refactoring".to_string(),
                tokens_used: 100,
                duration_ms: 500,
                error: None,
                status: crate::workflow::StepStatus::Completed,
                started_at: None,
                completed_at: None,
            },
        );
        results.insert(
            "build".to_string(),
            StepResult {
                step_name: "build".to_string(),
                response: String::new(),
                tokens_used: 0,
                duration_ms: 200,
                error: Some("compilation failed".to_string()),
                status: crate::workflow::StepStatus::Failed,
                started_at: None,
                completed_at: None,
            },
        );
        results
    }

    #[test]
    fn condition_always() {
        let results = make_results();
        assert!(evaluate_condition(&Condition::Always, &results));
    }

    #[test]
    fn condition_if_output_match() {
        let results = make_results();
        let cond = Condition::IfOutput {
            step: "analyze".to_string(),
            contains: "bugs".to_string(),
        };
        assert!(evaluate_condition(&cond, &results));
    }

    #[test]
    fn condition_if_output_no_match() {
        let results = make_results();
        let cond = Condition::IfOutput {
            step: "analyze".to_string(),
            contains: "perfect".to_string(),
        };
        assert!(!evaluate_condition(&cond, &results));
    }

    #[test]
    fn condition_if_output_missing_step() {
        let results = make_results();
        let cond = Condition::IfOutput {
            step: "nonexistent".to_string(),
            contains: "anything".to_string(),
        };
        assert!(!evaluate_condition(&cond, &results));
    }

    #[test]
    fn condition_if_success() {
        let results = make_results();
        assert!(evaluate_condition(
            &Condition::IfSuccess {
                step: "analyze".to_string()
            },
            &results
        ));
        assert!(!evaluate_condition(
            &Condition::IfSuccess {
                step: "build".to_string()
            },
            &results
        ));
    }

    #[test]
    fn condition_if_failure() {
        let results = make_results();
        assert!(!evaluate_condition(
            &Condition::IfFailure {
                step: "analyze".to_string()
            },
            &results
        ));
        assert!(evaluate_condition(
            &Condition::IfFailure {
                step: "build".to_string()
            },
            &results
        ));
    }

    #[test]
    fn condition_if_success_missing_step() {
        let results = make_results();
        assert!(!evaluate_condition(
            &Condition::IfSuccess {
                step: "nonexistent".to_string()
            },
            &results
        ));
    }

    #[test]
    fn expression_true_false_literals() {
        let results = HashMap::new();
        assert!(evaluate_condition(
            &Condition::Expression("true".to_string()),
            &results
        ));
        assert!(!evaluate_condition(
            &Condition::Expression("false".to_string()),
            &results
        ));
    }

    #[test]
    fn expression_step_success() {
        let results = make_results();
        assert!(evaluate_condition(
            &Condition::Expression("analyze.success".to_string()),
            &results
        ));
        assert!(!evaluate_condition(
            &Condition::Expression("build.success".to_string()),
            &results
        ));
    }

    #[test]
    fn expression_step_failed() {
        let results = make_results();
        assert!(evaluate_condition(
            &Condition::Expression("build.failed".to_string()),
            &results
        ));
        assert!(!evaluate_condition(
            &Condition::Expression("analyze.failed".to_string()),
            &results
        ));
    }

    #[test]
    fn expression_not() {
        let results = make_results();
        assert!(!evaluate_condition(
            &Condition::Expression("not analyze.success".to_string()),
            &results
        ));
        assert!(evaluate_condition(
            &Condition::Expression("not build.success".to_string()),
            &results
        ));
    }

    #[test]
    fn expression_and() {
        let results = make_results();
        assert!(!evaluate_condition(
            &Condition::Expression("analyze.success and build.success".to_string()),
            &results
        ));
        assert!(evaluate_condition(
            &Condition::Expression("analyze.success and build.failed".to_string()),
            &results
        ));
    }

    #[test]
    fn expression_or() {
        let results = make_results();
        assert!(evaluate_condition(
            &Condition::Expression("analyze.success or build.success".to_string()),
            &results
        ));
        assert!(!evaluate_condition(
            &Condition::Expression("analyze.failed or build.success".to_string()),
            &results
        ));
    }

    #[test]
    fn expression_output_contains() {
        let results = make_results();
        assert!(evaluate_condition(
            &Condition::Expression("analyze.output contains \"3 bugs\"".to_string()),
            &results
        ));
        assert!(!evaluate_condition(
            &Condition::Expression("analyze.output contains \"no issues\"".to_string()),
            &results
        ));
    }

    #[test]
    fn expression_unknown_defaults_false() {
        let results = make_results();
        assert!(!evaluate_condition(
            &Condition::Expression("unknown_garbage".to_string()),
            &results
        ));
    }

    #[test]
    fn condition_serialization_roundtrip() {
        let cond = Condition::IfOutput {
            step: "step1".to_string(),
            contains: "hello".to_string(),
        };
        let json = serde_json::to_string(&cond).expect("serialize");
        let deser: Condition = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cond, deser);
    }

    #[test]
    fn condition_always_serialization() {
        let cond = Condition::Always;
        let json = serde_json::to_string(&cond).expect("serialize");
        let deser: Condition = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cond, deser);
    }

    #[test]
    fn condition_expression_serialization() {
        let cond = Condition::Expression("step1.success and step2.failed".to_string());
        let json = serde_json::to_string(&cond).expect("serialize");
        let deser: Condition = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cond, deser);
    }
}
