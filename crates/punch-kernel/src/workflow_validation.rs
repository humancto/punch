//! Pre-execution validation for workflow DAGs.
//!
//! Performs cycle detection, unreachable step detection, missing dependency
//! detection, variable reference validation, and depth/breadth limits.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::workflow::DagWorkflowStep;

/// Maximum allowed depth of a DAG (longest path from any root to any leaf).
pub const MAX_DAG_DEPTH: usize = 100;

/// Maximum allowed number of steps in a workflow.
pub const MAX_DAG_BREADTH: usize = 1000;

/// A validation error found in a workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ValidationError {
    /// The DAG contains a cycle involving these steps.
    CycleDetected { steps: Vec<String> },
    /// A step declares a dependency that doesn't exist.
    MissingDependency { step: String, missing_dep: String },
    /// A step is unreachable (no path from any root).
    UnreachableStep { step: String },
    /// A variable reference points to a non-existent step.
    InvalidVariableRef { step: String, variable: String },
    /// Duplicate step names found.
    DuplicateStepName { name: String },
    /// The workflow has no steps.
    EmptyWorkflow,
    /// The DAG exceeds the maximum depth limit.
    ExceedsMaxDepth { depth: usize, limit: usize },
    /// The DAG exceeds the maximum breadth limit.
    ExceedsMaxBreadth { breadth: usize, limit: usize },
    /// An else_step references a non-existent step.
    InvalidElseStep { step: String, else_step: String },
    /// A fallback step references a non-existent step.
    InvalidFallbackStep { step: String, fallback: String },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CycleDetected { steps } => {
                write!(f, "cycle detected involving steps: {}", steps.join(" -> "))
            }
            Self::MissingDependency { step, missing_dep } => {
                write!(
                    f,
                    "step '{step}' depends on non-existent step '{missing_dep}'"
                )
            }
            Self::UnreachableStep { step } => {
                write!(f, "step '{step}' is unreachable from any root step")
            }
            Self::InvalidVariableRef { step, variable } => {
                write!(f, "step '{step}' references unknown variable '{variable}'")
            }
            Self::DuplicateStepName { name } => {
                write!(f, "duplicate step name: '{name}'")
            }
            Self::EmptyWorkflow => write!(f, "workflow has no steps"),
            Self::ExceedsMaxDepth { depth, limit } => {
                write!(f, "DAG depth {depth} exceeds limit {limit}")
            }
            Self::ExceedsMaxBreadth { breadth, limit } => {
                write!(f, "workflow has {breadth} steps, exceeding limit {limit}")
            }
            Self::InvalidElseStep { step, else_step } => {
                write!(
                    f,
                    "step '{step}' has else_step '{else_step}' which doesn't exist"
                )
            }
            Self::InvalidFallbackStep { step, fallback } => {
                write!(
                    f,
                    "step '{step}' has fallback '{fallback}' which doesn't exist"
                )
            }
        }
    }
}

/// Validate a workflow DAG, returning a list of all errors found.
///
/// Returns an empty vec if the workflow is valid.
pub fn validate_workflow(steps: &[DagWorkflowStep]) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // Empty workflow check
    if steps.is_empty() {
        errors.push(ValidationError::EmptyWorkflow);
        return errors;
    }

    // Breadth check
    if steps.len() > MAX_DAG_BREADTH {
        errors.push(ValidationError::ExceedsMaxBreadth {
            breadth: steps.len(),
            limit: MAX_DAG_BREADTH,
        });
    }

    // Build name set and check for duplicates
    let mut name_set: HashSet<&str> = HashSet::new();
    for step in steps {
        if !name_set.insert(&step.name) {
            errors.push(ValidationError::DuplicateStepName {
                name: step.name.clone(),
            });
        }
    }

    // Missing dependency check
    for step in steps {
        for dep in &step.depends_on {
            if !name_set.contains(dep.as_str()) {
                errors.push(ValidationError::MissingDependency {
                    step: step.name.clone(),
                    missing_dep: dep.clone(),
                });
            }
        }
    }

    // Else step check
    for step in steps {
        if let Some(ref else_step) = step.else_step
            && !name_set.contains(else_step.as_str())
        {
            errors.push(ValidationError::InvalidElseStep {
                step: step.name.clone(),
                else_step: else_step.clone(),
            });
        }
    }

    // Fallback step check
    for step in steps {
        if let Some(ref fallback) = step.fallback_step()
            && !name_set.contains(fallback.as_str())
        {
            errors.push(ValidationError::InvalidFallbackStep {
                step: step.name.clone(),
                fallback: fallback.clone(),
            });
        }
    }

    // Cycle detection via topological sort (Kahn's algorithm)
    let cycle_result = topological_sort(steps);
    match cycle_result {
        Ok(sorted) => {
            // Check depth
            let depth = compute_dag_depth(steps, &sorted);
            if depth > MAX_DAG_DEPTH {
                errors.push(ValidationError::ExceedsMaxDepth {
                    depth,
                    limit: MAX_DAG_DEPTH,
                });
            }

            // Unreachable step detection
            let reachable = find_reachable_steps(steps);
            for step in steps {
                if !reachable.contains(step.name.as_str()) {
                    errors.push(ValidationError::UnreachableStep {
                        step: step.name.clone(),
                    });
                }
            }
        }
        Err(cycle_steps) => {
            errors.push(ValidationError::CycleDetected { steps: cycle_steps });
        }
    }

    // Variable reference validation
    errors.extend(validate_variable_refs(steps, &name_set));

    errors
}

/// Perform topological sort using Kahn's algorithm.
///
/// Returns `Ok(sorted_names)` or `Err(cycle_participants)`.
pub fn topological_sort(steps: &[DagWorkflowStep]) -> Result<Vec<String>, Vec<String>> {
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();

    for step in steps {
        in_degree.entry(&step.name).or_insert(0);
        adjacency.entry(&step.name).or_default();
        for dep in &step.depends_on {
            if let Some(dep_step) = steps.iter().find(|s| s.name == *dep) {
                adjacency
                    .entry(&dep_step.name)
                    .or_default()
                    .push(&step.name);
                *in_degree.entry(&step.name).or_insert(0) += 1;
            }
        }
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(&name, _)| name)
        .collect();

    let mut sorted = Vec::new();

    while let Some(node) = queue.pop_front() {
        sorted.push(node.to_string());
        if let Some(neighbors) = adjacency.get(node) {
            for &neighbor in neighbors {
                if let Some(deg) = in_degree.get_mut(neighbor) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(neighbor);
                    }
                }
            }
        }
    }

    if sorted.len() == steps.len() {
        Ok(sorted)
    } else {
        // Find cycle participants: all nodes not in the sorted list
        let sorted_set: HashSet<&str> = sorted.iter().map(|s| s.as_str()).collect();
        let cycle_nodes: Vec<String> = steps
            .iter()
            .filter(|s| !sorted_set.contains(s.name.as_str()))
            .map(|s| s.name.clone())
            .collect();
        Err(cycle_nodes)
    }
}

/// Compute the longest path in the DAG (depth).
fn compute_dag_depth(steps: &[DagWorkflowStep], topo_order: &[String]) -> usize {
    let mut depth: HashMap<&str, usize> = HashMap::new();

    for name in topo_order {
        let step = steps.iter().find(|s| s.name == *name);
        let max_dep_depth = match step {
            Some(s) => s
                .depends_on
                .iter()
                .filter_map(|d| depth.get(d.as_str()))
                .copied()
                .max()
                .unwrap_or(0),
            None => 0,
        };
        depth.insert(name, max_dep_depth + 1);
    }

    depth.values().copied().max().unwrap_or(0)
}

/// Find all steps reachable from root steps (those with no dependencies).
fn find_reachable_steps(steps: &[DagWorkflowStep]) -> HashSet<&str> {
    let step_map: HashMap<&str, &DagWorkflowStep> =
        steps.iter().map(|s| (s.name.as_str(), s)).collect();

    // Build forward adjacency (dep -> dependents)
    let mut forward: HashMap<&str, Vec<&str>> = HashMap::new();
    for step in steps {
        forward.entry(&step.name).or_default();
        for dep in &step.depends_on {
            forward.entry(dep.as_str()).or_default().push(&step.name);
        }
    }

    // Root steps have no dependencies
    let roots: Vec<&str> = steps
        .iter()
        .filter(|s| s.depends_on.is_empty())
        .map(|s| s.name.as_str())
        .collect();

    let mut reachable: HashSet<&str> = HashSet::new();
    let mut queue: VecDeque<&str> = roots.into_iter().collect();

    while let Some(node) = queue.pop_front() {
        if reachable.insert(node) {
            if let Some(neighbors) = forward.get(node) {
                for &n in neighbors {
                    if !reachable.contains(n) {
                        queue.push_back(n);
                    }
                }
            }
            // Also follow else_step links
            if let Some(step) = step_map.get(node)
                && let Some(ref else_step) = step.else_step
                && !reachable.contains(else_step.as_str())
            {
                queue.push_back(else_step);
            }
        }
    }

    reachable
}

/// Validate that variable references like `{{step_name.output}}` point to real steps.
fn validate_variable_refs(
    steps: &[DagWorkflowStep],
    name_set: &HashSet<&str>,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    for step in steps {
        let template = &step.prompt_template;
        // Find all {{...}} patterns
        let mut pos = 0;
        while let Some(start) = template[pos..].find("{{") {
            let abs_start = pos + start + 2;
            if let Some(end) = template[abs_start..].find("}}") {
                let var_content = &template[abs_start..abs_start + end];
                // Check if it references a step output (step_name.output, step_name.status, etc.)
                if let Some(dot_pos) = var_content.find('.') {
                    let ref_step = &var_content[..dot_pos];
                    // Skip built-in variables
                    if ref_step != "loop" && ref_step != "step" && !name_set.contains(ref_step) {
                        errors.push(ValidationError::InvalidVariableRef {
                            step: step.name.clone(),
                            variable: var_content.to_string(),
                        });
                    }
                }
                pos = abs_start + end + 2;
            } else {
                break;
            }
        }
    }

    errors
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::{DagWorkflowStep, OnError};

    fn step(name: &str, deps: &[&str]) -> DagWorkflowStep {
        DagWorkflowStep {
            name: name.to_string(),
            fighter_name: "test".to_string(),
            prompt_template: "{{input}}".to_string(),
            timeout_secs: None,
            on_error: OnError::FailWorkflow,
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            condition: None,
            else_step: None,
            loop_config: None,
        }
    }

    #[test]
    fn validate_empty_workflow() {
        let errors = validate_workflow(&[]);
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], ValidationError::EmptyWorkflow));
    }

    #[test]
    fn validate_single_step() {
        let steps = vec![step("root", &[])];
        let errors = validate_workflow(&steps);
        assert!(errors.is_empty(), "errors: {errors:?}");
    }

    #[test]
    fn validate_linear_chain() {
        let steps = vec![step("a", &[]), step("b", &["a"]), step("c", &["b"])];
        let errors = validate_workflow(&steps);
        assert!(errors.is_empty(), "errors: {errors:?}");
    }

    #[test]
    fn validate_fan_out() {
        let steps = vec![
            step("root", &[]),
            step("b1", &["root"]),
            step("b2", &["root"]),
            step("b3", &["root"]),
        ];
        let errors = validate_workflow(&steps);
        assert!(errors.is_empty(), "errors: {errors:?}");
    }

    #[test]
    fn validate_fan_in() {
        let steps = vec![
            step("a", &[]),
            step("b", &[]),
            step("c", &[]),
            step("join", &["a", "b", "c"]),
        ];
        let errors = validate_workflow(&steps);
        assert!(errors.is_empty(), "errors: {errors:?}");
    }

    #[test]
    fn validate_diamond() {
        let steps = vec![
            step("root", &[]),
            step("left", &["root"]),
            step("right", &["root"]),
            step("join", &["left", "right"]),
        ];
        let errors = validate_workflow(&steps);
        assert!(errors.is_empty(), "errors: {errors:?}");
    }

    #[test]
    fn detect_cycle_simple() {
        let steps = vec![step("a", &["b"]), step("b", &["a"])];
        let errors = validate_workflow(&steps);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::CycleDetected { .. }))
        );
    }

    #[test]
    fn detect_cycle_three_way() {
        let steps = vec![step("a", &["c"]), step("b", &["a"]), step("c", &["b"])];
        let errors = validate_workflow(&steps);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::CycleDetected { .. }))
        );
    }

    #[test]
    fn detect_missing_dependency() {
        let steps = vec![step("a", &[]), step("b", &["nonexistent"])];
        let errors = validate_workflow(&steps);
        assert!(errors.iter().any(|e| matches!(
            e,
            ValidationError::MissingDependency {
                step,
                missing_dep
            } if step == "b" && missing_dep == "nonexistent"
        )));
    }

    #[test]
    fn detect_duplicate_step_name() {
        let steps = vec![step("dup", &[]), step("dup", &[])];
        let errors = validate_workflow(&steps);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::DuplicateStepName { name } if name == "dup"))
        );
    }

    #[test]
    fn detect_invalid_variable_ref() {
        let mut steps = vec![step("a", &[])];
        steps[0].prompt_template = "Use {{nonexistent.output}}".to_string();
        let errors = validate_workflow(&steps);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidVariableRef { .. }))
        );
    }

    #[test]
    fn valid_variable_ref_not_flagged() {
        let mut steps = vec![step("a", &[]), step("b", &["a"])];
        steps[1].prompt_template = "Use {{a.output}}".to_string();
        let errors = validate_workflow(&steps);
        assert!(
            errors.is_empty(),
            "should not flag valid refs, got: {errors:?}"
        );
    }

    #[test]
    fn loop_variable_not_flagged() {
        let mut steps = vec![step("a", &[])];
        steps[0].prompt_template = "Item {{loop.item}} at {{loop.index}}".to_string();
        let errors = validate_workflow(&steps);
        assert!(errors.is_empty(), "loop vars should be ignored: {errors:?}");
    }

    #[test]
    fn topological_sort_linear() {
        let steps = vec![step("a", &[]), step("b", &["a"]), step("c", &["b"])];
        let sorted = topological_sort(&steps).expect("should sort");
        let a_pos = sorted.iter().position(|s| s == "a").expect("a");
        let b_pos = sorted.iter().position(|s| s == "b").expect("b");
        let c_pos = sorted.iter().position(|s| s == "c").expect("c");
        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn topological_sort_diamond() {
        let steps = vec![
            step("root", &[]),
            step("left", &["root"]),
            step("right", &["root"]),
            step("join", &["left", "right"]),
        ];
        let sorted = topological_sort(&steps).expect("should sort");
        let root_pos = sorted.iter().position(|s| s == "root").expect("root");
        let left_pos = sorted.iter().position(|s| s == "left").expect("left");
        let right_pos = sorted.iter().position(|s| s == "right").expect("right");
        let join_pos = sorted.iter().position(|s| s == "join").expect("join");
        assert!(root_pos < left_pos);
        assert!(root_pos < right_pos);
        assert!(left_pos < join_pos);
        assert!(right_pos < join_pos);
    }

    #[test]
    fn topological_sort_cycle_returns_err() {
        let steps = vec![step("a", &["b"]), step("b", &["a"])];
        let result = topological_sort(&steps);
        assert!(result.is_err());
        let cycle = result.expect_err("cycle");
        assert!(cycle.contains(&"a".to_string()));
        assert!(cycle.contains(&"b".to_string()));
    }

    #[test]
    fn validation_error_display() {
        let err = ValidationError::CycleDetected {
            steps: vec!["a".to_string(), "b".to_string()],
        };
        let display = format!("{err}");
        assert!(display.contains("cycle detected"));
        assert!(display.contains("a -> b"));
    }

    #[test]
    fn validation_error_serialization() {
        let err = ValidationError::MissingDependency {
            step: "s1".to_string(),
            missing_dep: "s2".to_string(),
        };
        let json = serde_json::to_string(&err).expect("serialize");
        let deser: ValidationError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(err, deser);
    }

    #[test]
    fn detect_invalid_else_step() {
        let mut steps = vec![step("a", &[])];
        steps[0].else_step = Some("nonexistent".to_string());
        let errors = validate_workflow(&steps);
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidElseStep { .. }))
        );
    }

    #[test]
    fn valid_else_step_not_flagged() {
        let mut steps = vec![step("a", &[]), step("b", &[])];
        steps[0].else_step = Some("b".to_string());
        let errors = validate_workflow(&steps);
        assert!(
            !errors
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidElseStep { .. })),
            "valid else_step should not be flagged: {errors:?}"
        );
    }
}
