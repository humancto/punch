//! Multi-step agent workflow engine with DAG execution.
//!
//! The [`WorkflowEngine`] allows registering named workflows composed of
//! sequential steps or DAG-structured steps with parallel fan-out, conditional
//! branching, loops, and advanced error handling.
//!
//! ## Variable substitution
//!
//! Prompt templates support:
//! - `{{input}}` / `{{previous_output}}` — current pipeline input
//! - `{{step_name}}` — name of the current step
//! - `{{step_N}}` — output of step N (1-indexed, sequential mode)
//! - `{{some_step_name}}` — output of a step by name
//! - `{{step_name.output}}` — explicit step output reference
//! - `{{step_name.status}}` — step completion status
//! - `{{step_name.duration_ms}}` — step duration
//! - `{{loop.index}}` — current loop iteration
//! - `{{loop.item}}` — current loop item (ForEach)
//! - `{{step_name.output.field.nested}}` — JSON path into step output
//! - `{{step_name.output | uppercase}}` — data transformation

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, instrument, warn};
use uuid::Uuid;

use punch_memory::MemorySubstrate;
use punch_runtime::{FighterLoopParams, LlmDriver, run_fighter_loop, tools_for_capabilities};
use punch_types::{FighterId, FighterManifest, ModelConfig, PunchError, PunchResult, WeightClass};

use crate::workflow_conditions::{Condition, evaluate_condition};
use crate::workflow_loops::{LoopConfig, LoopState, calculate_backoff, parse_foreach_items};
use crate::workflow_validation::{ValidationError, topological_sort, validate_workflow};

// ---------------------------------------------------------------------------
// ID types
// ---------------------------------------------------------------------------

/// Unique identifier for a workflow definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkflowId(pub Uuid);

impl WorkflowId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for WorkflowId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for WorkflowId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a workflow run (execution instance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkflowRunId(pub Uuid);

impl WorkflowRunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for WorkflowRunId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for WorkflowRunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Workflow types
// ---------------------------------------------------------------------------

/// What to do when a workflow step fails.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum OnError {
    /// Abort the entire workflow.
    #[default]
    FailWorkflow,
    /// Skip the failed step and continue.
    SkipStep,
    /// Retry the step once, then fail if it fails again.
    RetryOnce,
    /// On error, run a fallback step instead.
    Fallback { step: String },
    /// Run an error handler step, then continue the workflow.
    CatchAndContinue { error_handler: String },
    /// Stop trying after N consecutive failures, with a cooldown.
    CircuitBreaker {
        max_failures: usize,
        cooldown_secs: u64,
    },
}

/// Per-step execution status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

impl std::fmt::Display for StepStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Skipped => write!(f, "skipped"),
            Self::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// A single step within a sequential workflow (legacy format, still supported).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    /// Human-readable name for this step.
    pub name: String,
    /// The fighter name to use for this step.
    pub fighter_name: String,
    /// Prompt template with variable substitution.
    pub prompt_template: String,
    /// Maximum time in seconds for this step (default 120).
    pub timeout_secs: Option<u64>,
    /// Error handling strategy.
    #[serde(default)]
    pub on_error: OnError,
}

/// A single step within a DAG workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagWorkflowStep {
    /// Human-readable name for this step (must be unique within the workflow).
    pub name: String,
    /// The fighter name to use for this step.
    pub fighter_name: String,
    /// Prompt template with variable substitution.
    pub prompt_template: String,
    /// Maximum time in seconds for this step (default 120).
    pub timeout_secs: Option<u64>,
    /// Error handling strategy.
    #[serde(default)]
    pub on_error: OnError,
    /// Steps that must complete before this one runs.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Optional condition — step is skipped if condition evaluates to false.
    #[serde(default)]
    pub condition: Option<Condition>,
    /// If condition is false, run this step instead (if/else branching).
    #[serde(default)]
    pub else_step: Option<String>,
    /// Optional loop configuration.
    #[serde(default)]
    pub loop_config: Option<LoopConfig>,
}

impl DagWorkflowStep {
    /// Extract the fallback step name from the on_error strategy, if any.
    pub fn fallback_step(&self) -> Option<String> {
        match &self.on_error {
            OnError::Fallback { step } => Some(step.clone()),
            OnError::CatchAndContinue { error_handler } => Some(error_handler.clone()),
            _ => None,
        }
    }
}

/// A workflow definition composed of sequential steps (legacy).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    /// Unique identifier.
    pub id: WorkflowId,
    /// Human-readable name.
    pub name: String,
    /// Ordered steps to execute.
    pub steps: Vec<WorkflowStep>,
}

/// A DAG workflow definition with parallel execution support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagWorkflow {
    /// Unique identifier.
    pub id: WorkflowId,
    /// Human-readable name.
    pub name: String,
    /// DAG steps (order in vec doesn't matter — execution order is determined by dependencies).
    pub steps: Vec<DagWorkflowStep>,
}

/// Status of a workflow run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    /// Some branches succeeded, some failed.
    PartiallyCompleted,
}

impl std::fmt::Display for WorkflowRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::PartiallyCompleted => write!(f, "partially_completed"),
        }
    }
}

/// Result of executing a single workflow step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    /// Name of the step.
    pub step_name: String,
    /// The response text from the fighter.
    pub response: String,
    /// Tokens consumed.
    pub tokens_used: u64,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Error message, if any.
    pub error: Option<String>,
    /// Per-step status.
    #[serde(default = "default_step_status")]
    pub status: StepStatus,
    /// When the step started executing.
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
    /// When the step finished executing.
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
}

fn default_step_status() -> StepStatus {
    StepStatus::Pending
}

/// A failed step result stored in the dead letter queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterEntry {
    /// The step name that failed.
    pub step_name: String,
    /// The error message.
    pub error: String,
    /// The input that was provided to the step.
    pub input: String,
    /// When the failure occurred.
    pub failed_at: DateTime<Utc>,
}

/// A single execution of a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRun {
    /// Unique run identifier.
    pub id: WorkflowRunId,
    /// The workflow that was executed.
    pub workflow_id: WorkflowId,
    /// Current status.
    pub status: WorkflowRunStatus,
    /// Results of each completed step.
    pub step_results: Vec<StepResult>,
    /// When the run started.
    pub started_at: DateTime<Utc>,
    /// When the run completed (or failed).
    pub completed_at: Option<DateTime<Utc>>,
    /// Dead letter queue for failed steps.
    #[serde(default)]
    pub dead_letters: Vec<DeadLetterEntry>,
    /// Execution trace showing which steps ran in parallel.
    #[serde(default)]
    pub execution_trace: Vec<ExecutionTraceEntry>,
}

/// An entry in the execution trace showing what happened at each "wave" of execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTraceEntry {
    /// Steps that executed in this wave (parallel batch).
    pub steps: Vec<String>,
    /// When this wave started.
    pub started_at: DateTime<Utc>,
    /// When this wave completed.
    pub completed_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Variable substitution
// ---------------------------------------------------------------------------

/// Replace template variables in a prompt string (sequential mode).
///
/// Supported variables:
/// - `{{input}}` — the current input (original input or previous step's output)
/// - `{{previous_output}}` — alias for `{{input}}`
/// - `{{step_name}}` — the name of the current step
/// - `{{step_1}}` / `{{step_N}}` — output of step N (1-indexed)
/// - `{{some_step_name}}` — output of a step referenced by its name
fn expand_variables(
    template: &str,
    current_input: &str,
    step_name: &str,
    step_results: &[StepResult],
) -> String {
    let mut result = template.to_string();

    // {{input}} and {{previous_output}} both resolve to the current pipeline input
    result = result.replace("{{input}}", current_input);
    result = result.replace("{{previous_output}}", current_input);

    // {{step_name}} resolves to the current step's name
    result = result.replace("{{step_name}}", step_name);

    // {{step_N}} resolves to the output of the Nth step (1-indexed)
    for (i, sr) in step_results.iter().enumerate() {
        let var = format!("{{{{step_{}}}}}", i + 1);
        result = result.replace(&var, &sr.response);
    }

    // {{step_result_name}} resolves to the output of a step by name
    for sr in step_results {
        let var = format!("{{{{{}}}}}", sr.step_name);
        result = result.replace(&var, &sr.response);
    }

    result
}

/// Replace template variables in a prompt string (DAG mode).
///
/// Supports all the sequential variables plus:
/// - `{{step_name.output}}` — explicit output reference
/// - `{{step_name.status}}` — step status
/// - `{{step_name.duration_ms}}` — step duration
/// - `{{loop.index}}` — current loop iteration
/// - `{{loop.item}}` — current loop item
/// - `{{step_name.output.field.nested}}` — JSON path
/// - `{{step_name.output | uppercase}}` — transformations
pub fn expand_dag_variables(
    template: &str,
    current_input: &str,
    step_name: &str,
    step_results: &HashMap<String, StepResult>,
    loop_state: Option<&LoopState>,
) -> String {
    let mut result = template.to_string();

    // Basic variables
    result = result.replace("{{input}}", current_input);
    result = result.replace("{{previous_output}}", current_input);
    result = result.replace("{{step_name}}", step_name);

    // Loop variables
    if let Some(ls) = loop_state {
        result = result.replace("{{loop.index}}", &ls.index.to_string());
        if let Some(ref item) = ls.item {
            result = result.replace("{{loop.item}}", item);
        }
    }

    // Process {{name.property}} and {{name.output.path}} patterns
    // We need to find all {{...}} patterns and resolve them
    let mut output = String::with_capacity(result.len());
    let mut remaining = result.as_str();

    while let Some(start) = remaining.find("{{") {
        output.push_str(&remaining[..start]);
        let after_start = &remaining[start + 2..];
        if let Some(end) = after_start.find("}}") {
            let var_content = &after_start[..end];
            let resolved = resolve_dag_variable(var_content, step_results);
            output.push_str(&resolved);
            remaining = &after_start[end + 2..];
        } else {
            output.push_str("{{");
            remaining = after_start;
        }
    }
    output.push_str(remaining);

    output
}

/// Resolve a single variable expression like `step_name.output` or `step_name.output | uppercase`.
fn resolve_dag_variable(var: &str, step_results: &HashMap<String, StepResult>) -> String {
    // Check for pipe transformation: `expr | transform`
    let (expr, transform) = if let Some(pipe_pos) = var.find(" | ") {
        let expr = var[..pipe_pos].trim();
        let transform = var[pipe_pos + 3..].trim();
        (expr, Some(transform))
    } else {
        (var.trim(), None)
    };

    // Resolve the expression
    let value = resolve_dag_expression(expr, step_results);

    // Apply transformation if present
    match transform {
        Some("uppercase") => value.to_uppercase(),
        Some("lowercase") => value.to_lowercase(),
        Some("trim") => value.trim().to_string(),
        Some("len") | Some("length") => value.len().to_string(),
        Some(t) if t.starts_with("json_extract ") => {
            let path = t
                .strip_prefix("json_extract ")
                .unwrap_or("")
                .trim_matches('"');
            json_path_extract(&value, path)
        }
        _ => value,
    }
}

/// Resolve a dotted expression like `step_name.output.field.nested`.
fn resolve_dag_expression(expr: &str, step_results: &HashMap<String, StepResult>) -> String {
    let parts: Vec<&str> = expr.splitn(2, '.').collect();
    if parts.len() < 2 {
        // Plain step name reference
        return step_results
            .get(parts[0])
            .map(|r| r.response.clone())
            .unwrap_or_else(|| format!("{{{{{expr}}}}}"));
    }

    let step_name = parts[0];
    let property = parts[1];

    let step_result = match step_results.get(step_name) {
        Some(r) => r,
        None => return format!("{{{{{expr}}}}}"),
    };

    match property {
        "output" => step_result.response.clone(),
        "status" => step_result.status.to_string(),
        "duration_ms" => step_result.duration_ms.to_string(),
        "error" => step_result
            .error
            .clone()
            .unwrap_or_else(|| "none".to_string()),
        _ if property.starts_with("output.") => {
            let json_path = property.strip_prefix("output.").unwrap_or("");
            json_path_extract(&step_result.response, json_path)
        }
        _ => format!("{{{{{expr}}}}}"),
    }
}

/// Extract a value from a JSON string using a dot-separated path.
///
/// Supports paths like `field`, `field.nested`, `$.key` (strips leading `$.`).
fn json_path_extract(json_str: &str, path: &str) -> String {
    let path = path.strip_prefix("$.").unwrap_or(path);
    let parsed: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return json_str.to_string(),
    };

    let mut current = &parsed;
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        match current.get(segment) {
            Some(v) => current = v,
            None => return String::new(),
        }
    }

    match current {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Circuit breaker state
// ---------------------------------------------------------------------------

/// Tracks circuit breaker state per-step across workflow runs.
#[derive(Debug, Clone, Default)]
pub struct CircuitBreakerState {
    /// Number of consecutive failures.
    pub consecutive_failures: usize,
    /// When the circuit was last tripped (entered open state).
    pub last_trip_time: Option<Instant>,
}

impl CircuitBreakerState {
    /// Check if the circuit is currently open (blocking execution).
    pub fn is_open(&self, max_failures: usize, cooldown_secs: u64) -> bool {
        if self.consecutive_failures < max_failures {
            return false;
        }
        // Check if cooldown has elapsed
        match self.last_trip_time {
            Some(trip_time) => trip_time.elapsed().as_secs() < cooldown_secs,
            None => true,
        }
    }

    /// Record a failure.
    pub fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        self.last_trip_time = Some(Instant::now());
    }

    /// Record a success, resetting the counter.
    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_trip_time = None;
    }
}


// ---------------------------------------------------------------------------
// DAG Executor (testable without LLM)
// ---------------------------------------------------------------------------

/// A step executor trait that allows testing the DAG engine without real LLM calls.
#[async_trait::async_trait]
pub trait StepExecutor: Send + Sync {
    /// Execute a single step and return its result.
    async fn execute(
        &self,
        step: &DagWorkflowStep,
        input: &str,
        step_results: &HashMap<String, StepResult>,
        loop_state: Option<&LoopState>,
    ) -> Result<StepResult, String>;
}

/// Execute a DAG workflow using the provided step executor.
///
/// This is the core DAG execution engine. Steps with no dependencies (roots) run
/// first. When a step completes, any step whose dependencies are now all satisfied
/// is scheduled. Steps with no mutual dependencies run concurrently using
/// `tokio::task::JoinSet` for true multi-threaded parallelism.
pub async fn execute_dag(
    workflow_name: &str,
    steps: &[DagWorkflowStep],
    input: &str,
    executor: Arc<dyn StepExecutor>,
) -> DagExecutionResult {
    // Validate first
    let validation_errors = validate_workflow(steps);
    if !validation_errors.is_empty() {
        return DagExecutionResult {
            status: WorkflowRunStatus::Failed,
            step_results: HashMap::new(),
            dead_letters: Vec::new(),
            execution_trace: Vec::new(),
            validation_errors,
        };
    }

    // Get topological order
    let topo_order = match topological_sort(steps) {
        Ok(order) => order,
        Err(_) => {
            return DagExecutionResult {
                status: WorkflowRunStatus::Failed,
                step_results: HashMap::new(),
                dead_letters: Vec::new(),
                execution_trace: Vec::new(),
                validation_errors: vec![ValidationError::CycleDetected {
                    steps: steps.iter().map(|s| s.name.clone()).collect(),
                }],
            };
        }
    };

    let step_map: HashMap<&str, &DagWorkflowStep> =
        steps.iter().map(|s| (s.name.as_str(), s)).collect();

    let mut completed: HashMap<String, StepResult> = HashMap::new();
    let mut dead_letters: Vec<DeadLetterEntry> = Vec::new();
    let mut execution_trace: Vec<ExecutionTraceEntry> = Vec::new();
    let mut circuit_breakers: HashMap<String, CircuitBreakerState> = HashMap::new();
    let mut skipped_steps: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut failed_steps: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Process in waves: each wave contains steps whose dependencies are all satisfied
    let mut remaining: Vec<String> = topo_order;

    while !remaining.is_empty() {
        // Find all steps that can run now (all deps satisfied)
        let (ready, not_ready): (Vec<String>, Vec<String>) =
            remaining.into_iter().partition(|name| {
                let step = match step_map.get(name.as_str()) {
                    Some(s) => s,
                    None => return false,
                };
                step.depends_on.iter().all(|dep| {
                    // A dependency is satisfied if it completed (not in failed_steps)
                    // or was explicitly skipped/handled
                    let is_done =
                        completed.contains_key(dep) || skipped_steps.contains(dep);
                    let is_blocking_failure = failed_steps.contains(dep);
                    is_done && !is_blocking_failure
                })
            });

        if ready.is_empty() {
            // No progress possible — remaining steps have unmet deps (likely due to failures)
            for name in &not_ready {
                skipped_steps.insert(name.clone());
                completed.insert(
                    name.clone(),
                    StepResult {
                        step_name: name.clone(),
                        response: String::new(),
                        tokens_used: 0,
                        duration_ms: 0,
                        error: Some("cancelled: unmet dependencies".to_string()),
                        status: StepStatus::Cancelled,
                        started_at: None,
                        completed_at: None,
                    },
                );
            }
            break;
        }

        remaining = not_ready;

        let wave_start = Utc::now();
        let wave_step_names: Vec<String> = ready.to_vec();

        // Execute all ready steps concurrently using tokio::task::JoinSet
        // for true multi-threaded parallelism.
        let mut wave_results: Vec<(String, Result<StepResult, String>, Option<String>)> =
            Vec::new();
        let mut join_set: tokio::task::JoinSet<(String, Result<StepResult, String>, Option<String>)> =
            tokio::task::JoinSet::new();

        for step_name in &wave_step_names {
            let step = match step_map.get(step_name.as_str()) {
                Some(s) => (*s).clone(),
                None => continue,
            };

            // Check condition
            let should_run = match &step.condition {
                Some(cond) => evaluate_condition(cond, &completed),
                None => true,
            };

            if !should_run {
                let else_step_name = step.else_step.clone();
                wave_results.push((
                    step_name.clone(),
                    Ok(StepResult {
                        step_name: step_name.clone(),
                        response: String::new(),
                        tokens_used: 0,
                        duration_ms: 0,
                        error: None,
                        status: StepStatus::Skipped,
                        started_at: Some(Utc::now()),
                        completed_at: Some(Utc::now()),
                    }),
                    else_step_name,
                ));
                continue;
            }

            // Check circuit breaker
            let cb_state = circuit_breakers
                .entry(step_name.clone())
                .or_default()
                .clone();
            if let OnError::CircuitBreaker {
                max_failures,
                cooldown_secs,
            } = &step.on_error
                && cb_state.is_open(*max_failures, *cooldown_secs)
            {
                wave_results.push((
                    step_name.clone(),
                    Ok(StepResult {
                        step_name: step_name.clone(),
                        response: String::new(),
                        tokens_used: 0,
                        duration_ms: 0,
                        error: Some("circuit breaker open".to_string()),
                        status: StepStatus::Failed,
                        started_at: Some(Utc::now()),
                        completed_at: Some(Utc::now()),
                    }),
                    None,
                ));
                continue;
            }

            let sn = step_name.clone();
            let completed_snapshot = completed.clone();
            let input_clone = input.to_string();
            let executor_clone = Arc::clone(&executor);

            join_set.spawn(async move {
                let result =
                    execute_step_with_loops(&step, &input_clone, &completed_snapshot, executor_clone.as_ref())
                        .await;
                (sn, result, None::<String>)
            });
        }

        // Wait for all spawned tasks to complete
        while let Some(join_result) = join_set.join_next().await {
            match join_result {
                Ok(task_result) => wave_results.push(task_result),
                Err(join_err) => {
                    // A JoinError means the task panicked or was cancelled
                    error!(error = %join_err, "spawned step task failed unexpectedly");
                }
            }
        }

        // Process results
        for (step_name, result, _else_step) in wave_results {
            match result {
                Ok(mut step_result) => {
                    if step_result.status == StepStatus::Skipped {
                        skipped_steps.insert(step_name.clone());
                        debug!(step = %step_name, workflow = %workflow_name, "step skipped (condition false)");
                    } else if step_result.error.is_some() {
                        failed_steps.insert(step_name.clone());
                        // Update circuit breaker
                        circuit_breakers
                            .entry(step_name.clone())
                            .or_default()
                            .record_failure();

                        let step = step_map.get(step_name.as_str());
                        if let Some(step) = step {
                            match &step.on_error {
                                OnError::Fallback { step: fb_step } => {
                                    // Try to execute fallback
                                    if let Some(fb) = step_map.get(fb_step.as_str()) {
                                        let fb_result = executor
                                            .execute(fb, input, &completed, None)
                                            .await;
                                        match fb_result {
                                            Ok(fb_res) => {
                                                step_result = fb_res;
                                                step_result.step_name = step_name.clone();
                                                failed_steps.remove(&step_name);
                                            }
                                            Err(fb_err) => {
                                                dead_letters.push(DeadLetterEntry {
                                                    step_name: step_name.clone(),
                                                    error: fb_err,
                                                    input: input.to_string(),
                                                    failed_at: Utc::now(),
                                                });
                                            }
                                        }
                                    }
                                }
                                OnError::CatchAndContinue { error_handler } => {
                                    // Run the error handler
                                    if let Some(handler) = step_map.get(error_handler.as_str()) {
                                        let _ = executor
                                            .execute(handler, input, &completed, None)
                                            .await;
                                    }
                                    // Continue anyway — mark as handled
                                    failed_steps.remove(&step_name);
                                }
                                OnError::SkipStep => {
                                    skipped_steps.insert(step_name.clone());
                                    failed_steps.remove(&step_name);
                                }
                                OnError::FailWorkflow => {
                                    dead_letters.push(DeadLetterEntry {
                                        step_name: step_name.clone(),
                                        error: step_result
                                            .error
                                            .clone()
                                            .unwrap_or_default(),
                                        input: input.to_string(),
                                        failed_at: Utc::now(),
                                    });
                                }
                                _ => {}
                            }
                        }
                    } else {
                        // Success
                        circuit_breakers
                            .entry(step_name.clone())
                            .or_default()
                            .record_success();
                        info!(step = %step_name, workflow = %workflow_name, "DAG step completed");
                    }
                    completed.insert(step_name, step_result);
                }
                Err(e) => {
                    failed_steps.insert(step_name.clone());
                    circuit_breakers
                        .entry(step_name.clone())
                        .or_default()
                        .record_failure();

                    let mut step_result = StepResult {
                        step_name: step_name.clone(),
                        response: String::new(),
                        tokens_used: 0,
                        duration_ms: 0,
                        error: Some(e.clone()),
                        status: StepStatus::Failed,
                        started_at: Some(Utc::now()),
                        completed_at: Some(Utc::now()),
                    };

                    // Try error recovery strategies
                    let step = step_map.get(step_name.as_str());
                    if let Some(step) = step {
                        match &step.on_error {
                            OnError::Fallback { step: fb_step } => {
                                if let Some(fb) = step_map.get(fb_step.as_str())
                                    && let Ok(fb_res) =
                                        executor.execute(fb, input, &completed, None).await
                                {
                                    step_result = fb_res;
                                    step_result.step_name = step_name.clone();
                                    step_result.error = None;
                                    step_result.status = StepStatus::Completed;
                                    failed_steps.remove(&step_name);
                                }
                            }
                            OnError::CatchAndContinue { error_handler } => {
                                if let Some(handler) =
                                    step_map.get(error_handler.as_str())
                                {
                                    let _ = executor
                                        .execute(handler, input, &completed, None)
                                        .await;
                                }
                                failed_steps.remove(&step_name);
                            }
                            OnError::SkipStep => {
                                step_result.status = StepStatus::Skipped;
                                skipped_steps.insert(step_name.clone());
                                failed_steps.remove(&step_name);
                            }
                            OnError::FailWorkflow => {
                                dead_letters.push(DeadLetterEntry {
                                    step_name: step_name.clone(),
                                    error: e,
                                    input: input.to_string(),
                                    failed_at: Utc::now(),
                                });
                            }
                            _ => {
                                dead_letters.push(DeadLetterEntry {
                                    step_name: step_name.clone(),
                                    error: e,
                                    input: input.to_string(),
                                    failed_at: Utc::now(),
                                });
                            }
                        }
                    } else {
                        dead_letters.push(DeadLetterEntry {
                            step_name: step_name.clone(),
                            error: e,
                            input: input.to_string(),
                            failed_at: Utc::now(),
                        });
                    }

                    completed.insert(step_name, step_result);
                }
            }
        }

        execution_trace.push(ExecutionTraceEntry {
            steps: wave_step_names,
            started_at: wave_start,
            completed_at: Some(Utc::now()),
        });
    }

    // Determine final status
    let has_failures = completed
        .values()
        .any(|r| r.status == StepStatus::Failed);
    let has_successes = completed
        .values()
        .any(|r| r.status == StepStatus::Completed);

    let status = if has_failures && has_successes {
        WorkflowRunStatus::PartiallyCompleted
    } else if has_failures {
        WorkflowRunStatus::Failed
    } else {
        WorkflowRunStatus::Completed
    };

    DagExecutionResult {
        status,
        step_results: completed,
        dead_letters,
        execution_trace,
        validation_errors: Vec::new(),
    }
}

/// Execute a step, handling loop configurations.
async fn execute_step_with_loops(
    step: &DagWorkflowStep,
    input: &str,
    completed: &HashMap<String, StepResult>,
    executor: &dyn StepExecutor,
) -> Result<StepResult, String> {
    match &step.loop_config {
        None => executor.execute(step, input, completed, None).await,
        Some(LoopConfig::ForEach {
            source_step,
            max_iterations,
        }) => {
            let source_output = completed
                .get(source_step)
                .map(|r| r.response.as_str())
                .unwrap_or("[]");
            let items = parse_foreach_items(source_output)?;
            let max = (*max_iterations).min(items.len());

            let mut loop_state = LoopState::new();
            let start = Utc::now();
            let instant = Instant::now();

            for (i, item) in items.into_iter().take(max).enumerate() {
                loop_state.index = i;
                loop_state.item = Some(item);

                let result = executor
                    .execute(step, input, completed, Some(&loop_state))
                    .await;

                match result {
                    Ok(r) => {
                        // Check for break/continue signals in output
                        if r.response.contains("__BREAK__") {
                            loop_state.push_result(r.response.replace("__BREAK__", ""));
                            break;
                        }
                        if r.response.contains("__CONTINUE__") {
                            continue;
                        }
                        loop_state.push_result(r.response);
                    }
                    Err(e) => return Err(e),
                }
            }

            let combined = loop_state.accumulated_results.join("\n");
            Ok(StepResult {
                step_name: step.name.clone(),
                response: combined,
                tokens_used: 0,
                duration_ms: instant.elapsed().as_millis() as u64,
                error: None,
                status: StepStatus::Completed,
                started_at: Some(start),
                completed_at: Some(Utc::now()),
            })
        }
        Some(LoopConfig::While {
            condition,
            max_iterations,
        }) => {
            let mut loop_state = LoopState::new();
            let start = Utc::now();
            let instant = Instant::now();

            for i in 0..*max_iterations {
                // Evaluate the condition with current completed results
                // For while loops, we add the accumulated results as a synthetic step
                let mut extended = completed.clone();
                if !loop_state.accumulated_results.is_empty() {
                    extended.insert(
                        step.name.clone(),
                        StepResult {
                            step_name: step.name.clone(),
                            response: loop_state.accumulated_results.last().cloned().unwrap_or_default(),
                            tokens_used: 0,
                            duration_ms: 0,
                            error: None,
                            status: StepStatus::Completed,
                            started_at: None,
                            completed_at: None,
                        },
                    );
                }

                if !evaluate_condition(condition, &extended) {
                    break;
                }

                loop_state.index = i;
                let result = executor
                    .execute(step, input, &extended, Some(&loop_state))
                    .await;

                match result {
                    Ok(r) => {
                        if r.response.contains("__BREAK__") {
                            loop_state.push_result(r.response.replace("__BREAK__", ""));
                            break;
                        }
                        loop_state.push_result(r.response);
                    }
                    Err(e) => return Err(e),
                }
            }

            let combined = loop_state.accumulated_results.join("\n");
            Ok(StepResult {
                step_name: step.name.clone(),
                response: combined,
                tokens_used: 0,
                duration_ms: instant.elapsed().as_millis() as u64,
                error: None,
                status: StepStatus::Completed,
                started_at: Some(start),
                completed_at: Some(Utc::now()),
            })
        }
        Some(LoopConfig::Retry {
            max_retries,
            backoff_ms,
            backoff_multiplier,
        }) => {
            let start = Utc::now();
            let instant = Instant::now();
            let mut last_error = String::new();

            for attempt in 0..=*max_retries {
                if attempt > 0 {
                    let wait = calculate_backoff(attempt - 1, *backoff_ms, *backoff_multiplier);
                    tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
                }

                match executor.execute(step, input, completed, None).await {
                    Ok(r) => return Ok(r),
                    Err(e) => {
                        last_error = e;
                        warn!(step = %step.name, attempt = attempt + 1, "retry attempt failed");
                    }
                }
            }

            Ok(StepResult {
                step_name: step.name.clone(),
                response: String::new(),
                tokens_used: 0,
                duration_ms: instant.elapsed().as_millis() as u64,
                error: Some(last_error),
                status: StepStatus::Failed,
                started_at: Some(start),
                completed_at: Some(Utc::now()),
            })
        }
    }
}

/// Result of executing a DAG workflow.
#[derive(Debug, Clone)]
pub struct DagExecutionResult {
    /// Overall workflow status.
    pub status: WorkflowRunStatus,
    /// Per-step results keyed by step name.
    pub step_results: HashMap<String, StepResult>,
    /// Dead letter entries for failed steps.
    pub dead_letters: Vec<DeadLetterEntry>,
    /// Execution trace.
    pub execution_trace: Vec<ExecutionTraceEntry>,
    /// Validation errors (if any — non-empty means workflow didn't execute).
    pub validation_errors: Vec<ValidationError>,
}

// ---------------------------------------------------------------------------
// WorkflowEngine
// ---------------------------------------------------------------------------

/// Engine for registering and executing multi-step agent workflows.
pub struct WorkflowEngine {
    /// Registered workflow definitions (sequential).
    workflows: DashMap<WorkflowId, Workflow>,
    /// Registered DAG workflow definitions.
    dag_workflows: DashMap<WorkflowId, DagWorkflow>,
    /// Workflow execution runs.
    runs: DashMap<WorkflowRunId, WorkflowRun>,
}

impl WorkflowEngine {
    /// Create a new workflow engine.
    pub fn new() -> Self {
        Self {
            workflows: DashMap::new(),
            dag_workflows: DashMap::new(),
            runs: DashMap::new(),
        }
    }

    /// Register a sequential workflow definition and return its ID.
    pub fn register_workflow(&self, workflow: Workflow) -> WorkflowId {
        let id = workflow.id;
        info!(workflow_id = %id, name = %workflow.name, "workflow registered");
        self.workflows.insert(id, workflow);
        id
    }

    /// Register a DAG workflow definition and return its ID.
    ///
    /// Validates the workflow before registering. Returns an error with
    /// validation details if the workflow is invalid.
    pub fn register_dag_workflow(
        &self,
        workflow: DagWorkflow,
    ) -> Result<WorkflowId, Vec<ValidationError>> {
        let errors = validate_workflow(&workflow.steps);
        if !errors.is_empty() {
            return Err(errors);
        }
        let id = workflow.id;
        info!(workflow_id = %id, name = %workflow.name, "DAG workflow registered");
        self.dag_workflows.insert(id, workflow);
        Ok(id)
    }

    /// Execute a sequential workflow with the given input string.
    #[instrument(skip(self, input, memory, driver, model_config), fields(%workflow_id))]
    pub async fn execute_workflow(
        &self,
        workflow_id: &WorkflowId,
        input: String,
        memory: Arc<MemorySubstrate>,
        driver: Arc<dyn LlmDriver>,
        model_config: &ModelConfig,
    ) -> PunchResult<WorkflowRunId> {
        let workflow = self
            .workflows
            .get(workflow_id)
            .ok_or_else(|| PunchError::Internal(format!("workflow {} not found", workflow_id)))?
            .clone();

        let run_id = WorkflowRunId::new();
        let run = WorkflowRun {
            id: run_id,
            workflow_id: *workflow_id,
            status: WorkflowRunStatus::Running,
            step_results: Vec::new(),
            started_at: Utc::now(),
            completed_at: None,
            dead_letters: Vec::new(),
            execution_trace: Vec::new(),
        };
        self.runs.insert(run_id, run);

        let mut current_input = input.clone();
        let mut step_results: Vec<StepResult> = Vec::new();
        let mut failed = false;

        for step in &workflow.steps {
            let result = self
                .execute_single_step(
                    step,
                    &workflow.name,
                    &current_input,
                    &step_results,
                    &memory,
                    &driver,
                    model_config,
                )
                .await;

            match result {
                Ok(step_result) => {
                    current_input = step_result.response.clone();
                    step_results.push(step_result);
                }
                Err(e) => {
                    let error_msg = format!("{e}");
                    match step.on_error {
                        OnError::SkipStep => {
                            warn!(step = %step.name, error = %error_msg, "step failed, skipping");
                            let skip_result = StepResult {
                                step_name: step.name.clone(),
                                response: String::new(),
                                tokens_used: 0,
                                duration_ms: 0,
                                error: Some(error_msg),
                                status: StepStatus::Skipped,
                                started_at: None,
                                completed_at: None,
                            };
                            step_results.push(skip_result);
                            continue;
                        }
                        OnError::RetryOnce => {
                            warn!(step = %step.name, error = %error_msg, "step failed, retrying once");
                            let retry_result = self
                                .execute_single_step(
                                    step,
                                    &workflow.name,
                                    &current_input,
                                    &step_results,
                                    &memory,
                                    &driver,
                                    model_config,
                                )
                                .await;

                            match retry_result {
                                Ok(step_result) => {
                                    current_input = step_result.response.clone();
                                    step_results.push(step_result);
                                }
                                Err(retry_err) => {
                                    error!(step = %step.name, error = %retry_err, "step failed on retry");
                                    let fail_result = StepResult {
                                        step_name: step.name.clone(),
                                        response: String::new(),
                                        tokens_used: 0,
                                        duration_ms: 0,
                                        error: Some(format!("{retry_err}")),
                                        status: StepStatus::Failed,
                                        started_at: None,
                                        completed_at: None,
                                    };
                                    step_results.push(fail_result);
                                    failed = true;
                                    break;
                                }
                            }
                        }
                        OnError::FailWorkflow => {
                            error!(step = %step.name, error = %error_msg, "step failed, aborting workflow");
                            let fail_result = StepResult {
                                step_name: step.name.clone(),
                                response: String::new(),
                                tokens_used: 0,
                                duration_ms: 0,
                                error: Some(error_msg),
                                status: StepStatus::Failed,
                                started_at: None,
                                completed_at: None,
                            };
                            step_results.push(fail_result);
                            failed = true;
                            break;
                        }
                        _ => {
                            // Fallback/CatchAndContinue/CircuitBreaker in sequential mode
                            // just fail the workflow for now
                            let fail_result = StepResult {
                                step_name: step.name.clone(),
                                response: String::new(),
                                tokens_used: 0,
                                duration_ms: 0,
                                error: Some(error_msg),
                                status: StepStatus::Failed,
                                started_at: None,
                                completed_at: None,
                            };
                            step_results.push(fail_result);
                            failed = true;
                            break;
                        }
                    }
                }
            }
        }

        // Update the run with results.
        if let Some(mut run) = self.runs.get_mut(&run_id) {
            run.step_results = step_results;
            run.status = if failed {
                WorkflowRunStatus::Failed
            } else {
                WorkflowRunStatus::Completed
            };
            run.completed_at = Some(Utc::now());
        }

        Ok(run_id)
    }

    /// Execute a single workflow step, creating a temporary fighter and running
    /// it through the fighter loop.
    #[allow(clippy::too_many_arguments)]
    async fn execute_single_step(
        &self,
        step: &WorkflowStep,
        workflow_name: &str,
        current_input: &str,
        step_results: &[StepResult],
        memory: &Arc<MemorySubstrate>,
        driver: &Arc<dyn LlmDriver>,
        model_config: &ModelConfig,
    ) -> PunchResult<StepResult> {
        let step_start = Instant::now();
        let started_at = Utc::now();

        // Substitute variables in the prompt template.
        let prompt = expand_variables(
            &step.prompt_template,
            current_input,
            &step.name,
            step_results,
        );

        // Create a temporary fighter for this step.
        let fighter_id = FighterId::new();
        let fighter_manifest = FighterManifest {
            name: step.fighter_name.clone(),
            description: format!("Workflow step: {}", step.name),
            model: model_config.clone(),
            system_prompt: format!(
                "You are executing step '{}' of workflow '{}'.",
                step.name, workflow_name
            ),
            capabilities: Vec::new(),
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        };

        // Save the fighter and create a bout.
        if let Err(e) = memory
            .save_fighter(
                &fighter_id,
                &fighter_manifest,
                punch_types::FighterStatus::Idle,
            )
            .await
        {
            error!(error = %e, "failed to persist workflow fighter");
        }

        let bout_id = memory.create_bout(&fighter_id).await.map_err(|e| {
            PunchError::Internal(format!(
                "failed to create bout for step '{}': {e}",
                step.name
            ))
        })?;

        let available_tools = tools_for_capabilities(&fighter_manifest.capabilities);
        let timeout_secs = step.timeout_secs.unwrap_or(120);

        let params = FighterLoopParams {
            manifest: fighter_manifest,
            user_message: prompt,
            bout_id,
            fighter_id,
            memory: Arc::clone(memory),
            driver: Arc::clone(driver),
            available_tools,
            max_iterations: Some(20),
            context_window: None,
            tool_timeout_secs: Some(timeout_secs),
            coordinator: None,
            approval_engine: None,
            sandbox: None,
        };

        let loop_result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            run_fighter_loop(params),
        )
        .await;

        match loop_result {
            Ok(Ok(result)) => {
                let step_result = StepResult {
                    step_name: step.name.clone(),
                    response: result.response,
                    tokens_used: result.usage.total(),
                    duration_ms: step_start.elapsed().as_millis() as u64,
                    error: None,
                    status: StepStatus::Completed,
                    started_at: Some(started_at),
                    completed_at: Some(Utc::now()),
                };
                info!(step = %step.name, tokens = step_result.tokens_used, "workflow step completed");
                Ok(step_result)
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(PunchError::Internal(format!(
                "step '{}' timed out after {}s",
                step.name, timeout_secs
            ))),
        }
    }

    /// Get a workflow run by its ID.
    pub fn get_run(&self, run_id: &WorkflowRunId) -> Option<WorkflowRun> {
        self.runs.get(run_id).map(|r| r.clone())
    }

    /// List all registered sequential workflows.
    pub fn list_workflows(&self) -> Vec<Workflow> {
        self.workflows.iter().map(|w| w.value().clone()).collect()
    }

    /// List all registered DAG workflows.
    pub fn list_dag_workflows(&self) -> Vec<DagWorkflow> {
        self.dag_workflows
            .iter()
            .map(|w| w.value().clone())
            .collect()
    }

    /// List all workflow runs.
    pub fn list_runs(&self) -> Vec<WorkflowRun> {
        self.runs.iter().map(|r| r.value().clone()).collect()
    }

    /// List workflow runs filtered by workflow ID.
    pub fn list_runs_for_workflow(&self, workflow_id: &WorkflowId) -> Vec<WorkflowRun> {
        self.runs
            .iter()
            .filter(|r| r.value().workflow_id == *workflow_id)
            .map(|r| r.value().clone())
            .collect()
    }

    /// Get a sequential workflow by its ID.
    pub fn get_workflow(&self, id: &WorkflowId) -> Option<Workflow> {
        self.workflows.get(id).map(|w| w.clone())
    }

    /// Get a DAG workflow by its ID.
    pub fn get_dag_workflow(&self, id: &WorkflowId) -> Option<DagWorkflow> {
        self.dag_workflows.get(id).map(|w| w.clone())
    }
}

impl Default for WorkflowEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    // A mock step executor for testing
    struct MockExecutor {
        /// Map of step name -> response
        responses: HashMap<String, String>,
        /// Steps that should fail
        failing_steps: HashMap<String, String>,
        /// Track execution count per step
        execution_counts: DashMap<String, AtomicUsize>,
    }

    impl MockExecutor {
        fn new() -> Self {
            Self {
                responses: HashMap::new(),
                failing_steps: HashMap::new(),
                execution_counts: DashMap::new(),
            }
        }

        fn with_response(mut self, step: &str, response: &str) -> Self {
            self.responses
                .insert(step.to_string(), response.to_string());
            self
        }

        fn with_failure(mut self, step: &str, error: &str) -> Self {
            self.failing_steps
                .insert(step.to_string(), error.to_string());
            self
        }

        #[allow(dead_code)]
        fn execution_count(&self, step: &str) -> usize {
            self.execution_counts
                .get(step)
                .map(|c| c.load(Ordering::Relaxed))
                .unwrap_or(0)
        }
    }

    #[async_trait::async_trait]
    impl StepExecutor for MockExecutor {
        async fn execute(
            &self,
            step: &DagWorkflowStep,
            input: &str,
            step_results: &HashMap<String, StepResult>,
            loop_state: Option<&LoopState>,
        ) -> Result<StepResult, String> {
            // Track execution
            self.execution_counts
                .entry(step.name.clone())
                .or_insert_with(|| AtomicUsize::new(0))
                .fetch_add(1, Ordering::Relaxed);

            // Check if step should fail
            if let Some(err) = self.failing_steps.get(&step.name) {
                return Err(err.clone());
            }

            let prompt = expand_dag_variables(
                &step.prompt_template,
                input,
                &step.name,
                step_results,
                loop_state,
            );

            let response = self
                .responses
                .get(&step.name)
                .cloned()
                .unwrap_or(prompt);

            Ok(StepResult {
                step_name: step.name.clone(),
                response,
                tokens_used: 10,
                duration_ms: 5,
                error: None,
                status: StepStatus::Completed,
                started_at: Some(Utc::now()),
                completed_at: Some(Utc::now()),
            })
        }
    }

    /// A mock executor that adds a delay to simulate real execution time.
    struct TimedMockExecutor {
        delay_ms: u64,
    }

    #[async_trait::async_trait]
    impl StepExecutor for TimedMockExecutor {
        async fn execute(
            &self,
            step: &DagWorkflowStep,
            _input: &str,
            _step_results: &HashMap<String, StepResult>,
            _loop_state: Option<&LoopState>,
        ) -> Result<StepResult, String> {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            Ok(StepResult {
                step_name: step.name.clone(),
                response: format!("done-{}", step.name),
                tokens_used: 10,
                duration_ms: self.delay_ms,
                error: None,
                status: StepStatus::Completed,
                started_at: Some(Utc::now()),
                completed_at: Some(Utc::now()),
            })
        }
    }

    /// A mock executor that fails the first N attempts for a step.
    struct FailNTimesMockExecutor {
        fail_count: usize,
        attempts: DashMap<String, AtomicUsize>,
    }

    impl FailNTimesMockExecutor {
        fn new(fail_count: usize) -> Self {
            Self {
                fail_count,
                attempts: DashMap::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl StepExecutor for FailNTimesMockExecutor {
        async fn execute(
            &self,
            step: &DagWorkflowStep,
            _input: &str,
            _step_results: &HashMap<String, StepResult>,
            _loop_state: Option<&LoopState>,
        ) -> Result<StepResult, String> {
            let attempt = self
                .attempts
                .entry(step.name.clone())
                .or_insert_with(|| AtomicUsize::new(0))
                .fetch_add(1, Ordering::Relaxed);

            if attempt < self.fail_count {
                return Err(format!("failure attempt {}", attempt + 1));
            }

            Ok(StepResult {
                step_name: step.name.clone(),
                response: format!("success on attempt {}", attempt + 1),
                tokens_used: 10,
                duration_ms: 5,
                error: None,
                status: StepStatus::Completed,
                started_at: Some(Utc::now()),
                completed_at: Some(Utc::now()),
            })
        }
    }

    fn dag_step(name: &str, deps: &[&str]) -> DagWorkflowStep {
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

    // ---- Existing sequential tests (preserved) ----

    #[test]
    fn register_and_list_workflows() {
        let engine = WorkflowEngine::new();

        let workflow = Workflow {
            id: WorkflowId::new(),
            name: "test-workflow".to_string(),
            steps: vec![
                WorkflowStep {
                    name: "step1".to_string(),
                    fighter_name: "analyzer".to_string(),
                    prompt_template: "Analyze: {{input}}".to_string(),
                    timeout_secs: None,
                    on_error: OnError::FailWorkflow,
                },
                WorkflowStep {
                    name: "step2".to_string(),
                    fighter_name: "summarizer".to_string(),
                    prompt_template: "Summarize the analysis: {{step1}}".to_string(),
                    timeout_secs: Some(60),
                    on_error: OnError::SkipStep,
                },
            ],
        };

        let id = engine.register_workflow(workflow);
        let workflows = engine.list_workflows();
        assert_eq!(workflows.len(), 1);
        assert_eq!(workflows[0].name, "test-workflow");
        assert_eq!(workflows[0].steps.len(), 2);

        let fetched = engine.get_workflow(&id).expect("workflow should exist");
        assert_eq!(fetched.name, "test-workflow");
    }

    #[test]
    fn variable_substitution_basic() {
        let result = expand_variables(
            "Analyze {{input}} for step {{step_name}}",
            "hello world",
            "analysis",
            &[],
        );
        assert_eq!(result, "Analyze hello world for step analysis");
    }

    #[test]
    fn variable_substitution_previous_output() {
        let result = expand_variables(
            "Continue from: {{previous_output}}",
            "step 1 output",
            "step2",
            &[],
        );
        assert_eq!(result, "Continue from: step 1 output");
    }

    #[test]
    fn variable_substitution_step_refs() {
        let step_results = vec![
            StepResult {
                step_name: "analyze".to_string(),
                response: "analysis result".to_string(),
                tokens_used: 100,
                duration_ms: 500,
                error: None,
                status: StepStatus::Completed,
                started_at: None,
                completed_at: None,
            },
            StepResult {
                step_name: "review".to_string(),
                response: "review result".to_string(),
                tokens_used: 80,
                duration_ms: 400,
                error: None,
                status: StepStatus::Completed,
                started_at: None,
                completed_at: None,
            },
        ];

        let result = expand_variables(
            "Step 1 said: {{step_1}}, Step 2 said: {{step_2}}",
            "current",
            "step3",
            &step_results,
        );
        assert_eq!(
            result,
            "Step 1 said: analysis result, Step 2 said: review result"
        );

        let result = expand_variables(
            "Analysis: {{analyze}}, Review: {{review}}",
            "current",
            "step3",
            &step_results,
        );
        assert_eq!(result, "Analysis: analysis result, Review: review result");
    }

    #[test]
    fn workflow_run_status_display() {
        assert_eq!(WorkflowRunStatus::Pending.to_string(), "pending");
        assert_eq!(WorkflowRunStatus::Running.to_string(), "running");
        assert_eq!(WorkflowRunStatus::Completed.to_string(), "completed");
        assert_eq!(WorkflowRunStatus::Failed.to_string(), "failed");
        assert_eq!(
            WorkflowRunStatus::PartiallyCompleted.to_string(),
            "partially_completed"
        );
    }

    #[test]
    fn get_nonexistent_run_returns_none() {
        let engine = WorkflowEngine::new();
        let run_id = WorkflowRunId::new();
        assert!(engine.get_run(&run_id).is_none());
    }

    #[test]
    fn get_nonexistent_workflow_returns_none() {
        let engine = WorkflowEngine::new();
        let id = WorkflowId::new();
        assert!(engine.get_workflow(&id).is_none());
    }

    #[test]
    fn workflow_engine_default() {
        let engine = WorkflowEngine::default();
        assert!(engine.list_workflows().is_empty());
        assert!(engine.list_runs().is_empty());
    }

    #[test]
    fn register_multiple_workflows() {
        let engine = WorkflowEngine::new();

        for i in 0..5 {
            let workflow = Workflow {
                id: WorkflowId::new(),
                name: format!("workflow-{}", i),
                steps: vec![],
            };
            engine.register_workflow(workflow);
        }

        assert_eq!(engine.list_workflows().len(), 5);
    }

    #[test]
    fn register_workflow_returns_correct_id() {
        let engine = WorkflowEngine::new();
        let wf_id = WorkflowId::new();
        let workflow = Workflow {
            id: wf_id,
            name: "id-test".to_string(),
            steps: vec![],
        };
        let returned_id = engine.register_workflow(workflow);
        assert_eq!(returned_id, wf_id);
    }

    #[test]
    fn workflow_id_display() {
        let id = WorkflowId::new();
        let s = format!("{}", id);
        assert!(!s.is_empty());
    }

    #[test]
    fn workflow_run_id_display() {
        let id = WorkflowRunId::new();
        let s = format!("{}", id);
        assert!(!s.is_empty());
    }

    #[test]
    fn workflow_id_default() {
        let id = WorkflowId::default();
        assert!(!id.0.is_nil());
    }

    #[test]
    fn workflow_run_id_default() {
        let id = WorkflowRunId::default();
        assert!(!id.0.is_nil());
    }

    #[test]
    fn variable_substitution_no_variables() {
        let result = expand_variables("plain text with no vars", "input", "step", &[]);
        assert_eq!(result, "plain text with no vars");
    }

    #[test]
    fn variable_substitution_all_variables_at_once() {
        let step_results = vec![StepResult {
            step_name: "analysis".to_string(),
            response: "analyzed data".to_string(),
            tokens_used: 50,
            duration_ms: 100,
            error: None,
            status: StepStatus::Completed,
            started_at: None,
            completed_at: None,
        }];

        let result = expand_variables(
            "Input: {{input}}, Prev: {{previous_output}}, Step: {{step_name}}, S1: {{step_1}}, Named: {{analysis}}",
            "my input",
            "current_step",
            &step_results,
        );
        assert_eq!(
            result,
            "Input: my input, Prev: my input, Step: current_step, S1: analyzed data, Named: analyzed data"
        );
    }

    #[test]
    fn variable_substitution_empty_input() {
        let result = expand_variables("{{input}} is here", "", "step", &[]);
        assert_eq!(result, " is here");
    }

    #[test]
    fn variable_substitution_multiple_same_var() {
        let result = expand_variables(
            "{{input}} and {{input}} again",
            "hello",
            "step",
            &[],
        );
        assert_eq!(result, "hello and hello again");
    }

    #[test]
    fn on_error_default_is_fail_workflow() {
        let on_error = OnError::default();
        assert!(matches!(on_error, OnError::FailWorkflow));
    }

    #[test]
    fn list_runs_for_workflow_filters_correctly() {
        let engine = WorkflowEngine::new();
        let wf_id_1 = WorkflowId::new();
        let wf_id_2 = WorkflowId::new();

        assert!(engine.list_runs_for_workflow(&wf_id_1).is_empty());
        assert!(engine.list_runs_for_workflow(&wf_id_2).is_empty());
    }

    #[test]
    fn workflow_step_serialization() {
        let step = WorkflowStep {
            name: "test".to_string(),
            fighter_name: "fighter".to_string(),
            prompt_template: "Do {{input}}".to_string(),
            timeout_secs: Some(30),
            on_error: OnError::SkipStep,
        };
        let json = serde_json::to_string(&step).expect("serialize");
        let deserialized: WorkflowStep = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.name, "test");
        assert_eq!(deserialized.timeout_secs, Some(30));
    }

    #[test]
    fn workflow_serialization_roundtrip() {
        let workflow = Workflow {
            id: WorkflowId::new(),
            name: "roundtrip".to_string(),
            steps: vec![WorkflowStep {
                name: "s1".to_string(),
                fighter_name: "f1".to_string(),
                prompt_template: "{{input}}".to_string(),
                timeout_secs: None,
                on_error: OnError::RetryOnce,
            }],
        };
        let json = serde_json::to_string(&workflow).expect("serialize");
        let deserialized: Workflow = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.name, "roundtrip");
        assert_eq!(deserialized.steps.len(), 1);
    }

    #[test]
    fn step_result_with_error() {
        let sr = StepResult {
            step_name: "failing".to_string(),
            response: String::new(),
            tokens_used: 0,
            duration_ms: 0,
            error: Some("timeout".to_string()),
            status: StepStatus::Failed,
            started_at: None,
            completed_at: None,
        };
        assert!(sr.error.is_some());
        assert_eq!(sr.error.expect("error"), "timeout");
    }

    #[test]
    fn variable_substitution_step_ref_by_number_out_of_range() {
        let step_results = vec![
            StepResult {
                step_name: "a".to_string(),
                response: "r1".to_string(),
                tokens_used: 0,
                duration_ms: 0,
                error: None,
                status: StepStatus::Completed,
                started_at: None,
                completed_at: None,
            },
            StepResult {
                step_name: "b".to_string(),
                response: "r2".to_string(),
                tokens_used: 0,
                duration_ms: 0,
                error: None,
                status: StepStatus::Completed,
                started_at: None,
                completed_at: None,
            },
        ];
        let result = expand_variables("{{step_5}}", "input", "step", &step_results);
        assert_eq!(result, "{{step_5}}");
    }

    // ---- New DAG tests ----

    #[tokio::test]
    async fn dag_linear_execution() {
        let steps = vec![
            dag_step("a", &[]),
            dag_step("b", &["a"]),
            dag_step("c", &["b"]),
        ];
        let executor = MockExecutor::new()
            .with_response("a", "result_a")
            .with_response("b", "result_b")
            .with_response("c", "result_c");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert_eq!(result.step_results.len(), 3);
        assert_eq!(result.step_results["a"].response, "result_a");
        assert_eq!(result.step_results["b"].response, "result_b");
        assert_eq!(result.step_results["c"].response, "result_c");
    }

    #[tokio::test]
    async fn dag_fan_out_execution() {
        let steps = vec![
            dag_step("root", &[]),
            dag_step("branch1", &["root"]),
            dag_step("branch2", &["root"]),
            dag_step("branch3", &["root"]),
        ];
        let executor = MockExecutor::new()
            .with_response("root", "root_out")
            .with_response("branch1", "b1_out")
            .with_response("branch2", "b2_out")
            .with_response("branch3", "b3_out");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert_eq!(result.step_results.len(), 4);
        // All branches should have completed
        assert_eq!(result.step_results["branch1"].response, "b1_out");
        assert_eq!(result.step_results["branch2"].response, "b2_out");
        assert_eq!(result.step_results["branch3"].response, "b3_out");
    }

    #[tokio::test]
    async fn dag_fan_in_execution() {
        let steps = vec![
            dag_step("a", &[]),
            dag_step("b", &[]),
            dag_step("c", &[]),
            dag_step("join", &["a", "b", "c"]),
        ];
        let executor = MockExecutor::new()
            .with_response("a", "ra")
            .with_response("b", "rb")
            .with_response("c", "rc")
            .with_response("join", "joined");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert_eq!(result.step_results["join"].response, "joined");
        // a, b, c should have run in the same wave (first trace entry)
        assert_eq!(result.execution_trace.len(), 2);
        let first_wave = &result.execution_trace[0].steps;
        assert!(first_wave.contains(&"a".to_string()));
        assert!(first_wave.contains(&"b".to_string()));
        assert!(first_wave.contains(&"c".to_string()));
    }

    #[tokio::test]
    async fn dag_diamond_execution() {
        let steps = vec![
            dag_step("root", &[]),
            dag_step("left", &["root"]),
            dag_step("right", &["root"]),
            dag_step("join", &["left", "right"]),
        ];
        let executor = MockExecutor::new()
            .with_response("root", "root_out")
            .with_response("left", "left_out")
            .with_response("right", "right_out")
            .with_response("join", "joined");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert_eq!(result.step_results.len(), 4);
        // left and right should be in same wave
        let wave2 = &result.execution_trace[1].steps;
        assert!(wave2.contains(&"left".to_string()));
        assert!(wave2.contains(&"right".to_string()));
    }

    #[tokio::test]
    async fn dag_parallel_actually_concurrent() {
        // Steps a, b, c have no deps, each takes 50ms.
        // If run sequentially: ~150ms. If parallel: ~50ms.
        let steps = vec![dag_step("a", &[]), dag_step("b", &[]), dag_step("c", &[])];
        let executor = TimedMockExecutor { delay_ms: 50 };

        let start = Instant::now();
        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        let elapsed = start.elapsed();

        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert_eq!(result.step_results.len(), 3);
        // Should complete in roughly 50ms (parallel), not 150ms (sequential)
        // Use generous bound to avoid flakiness
        assert!(
            elapsed.as_millis() < 120,
            "parallel execution took {}ms, expected ~50ms",
            elapsed.as_millis()
        );
    }

    #[tokio::test]
    async fn dag_condition_if_success() {
        let mut steps = vec![dag_step("a", &[]), dag_step("b", &["a"])];
        steps[1].condition = Some(Condition::IfSuccess {
            step: "a".to_string(),
        });
        let executor = MockExecutor::new()
            .with_response("a", "ok")
            .with_response("b", "b_ran");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.step_results["b"].status, StepStatus::Completed);
        assert_eq!(result.step_results["b"].response, "b_ran");
    }

    #[tokio::test]
    async fn dag_condition_skips_step() {
        let mut steps = vec![dag_step("a", &[]), dag_step("b", &["a"])];
        steps[1].condition = Some(Condition::IfFailure {
            step: "a".to_string(),
        });
        let executor = MockExecutor::new()
            .with_response("a", "ok")
            .with_response("b", "should_not_run");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.step_results["b"].status, StepStatus::Skipped);
    }

    #[tokio::test]
    async fn dag_condition_if_output() {
        let mut steps = vec![dag_step("a", &[]), dag_step("b", &["a"])];
        steps[1].condition = Some(Condition::IfOutput {
            step: "a".to_string(),
            contains: "magic".to_string(),
        });
        let executor = MockExecutor::new()
            .with_response("a", "this has magic inside")
            .with_response("b", "b_ran");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.step_results["b"].status, StepStatus::Completed);
    }

    #[tokio::test]
    async fn dag_condition_if_output_no_match() {
        let mut steps = vec![dag_step("a", &[]), dag_step("b", &["a"])];
        steps[1].condition = Some(Condition::IfOutput {
            step: "a".to_string(),
            contains: "magic".to_string(),
        });
        let executor = MockExecutor::new()
            .with_response("a", "no special word here")
            .with_response("b", "should_not_run");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.step_results["b"].status, StepStatus::Skipped);
    }

    #[tokio::test]
    async fn dag_foreach_loop() {
        let mut steps = vec![dag_step("source", &[]), dag_step("process", &["source"])];
        steps[0].prompt_template = "{{input}}".to_string();
        steps[1].loop_config = Some(LoopConfig::ForEach {
            source_step: "source".to_string(),
            max_iterations: 100,
        });
        steps[1].prompt_template = "process item: {{loop.item}}".to_string();

        let executor = MockExecutor::new()
            .with_response("source", r#"["apple", "banana", "cherry"]"#);

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        let process_result = &result.step_results["process"];
        // Should have processed all 3 items
        assert!(
            process_result.response.contains("process item: apple"),
            "response: {}",
            process_result.response
        );
    }

    #[tokio::test]
    async fn dag_while_loop() {
        let mut steps = vec![dag_step("counter", &[])];
        steps[0].loop_config = Some(LoopConfig::While {
            condition: Condition::Expression("true".to_string()),
            max_iterations: 5,
        });

        let executor = MockExecutor::new().with_response("counter", "tick");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        let counter_result = &result.step_results["counter"];
        // Should have 5 "tick" entries
        let ticks: Vec<&str> = counter_result.response.split('\n').collect();
        assert_eq!(ticks.len(), 5);
    }

    #[tokio::test]
    async fn dag_retry_loop_succeeds_eventually() {
        let mut steps = vec![dag_step("flaky", &[])];
        steps[0].loop_config = Some(LoopConfig::Retry {
            max_retries: 3,
            backoff_ms: 1, // minimal backoff for tests
            backoff_multiplier: 1.0,
        });

        // Fails first 2 times, succeeds on 3rd
        let executor = FailNTimesMockExecutor::new(2);

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert!(result.step_results["flaky"].error.is_none());
        assert!(result.step_results["flaky"]
            .response
            .contains("success on attempt 3"));
    }

    #[tokio::test]
    async fn dag_retry_loop_exhausts_retries() {
        let mut steps = vec![dag_step("flaky", &[])];
        steps[0].loop_config = Some(LoopConfig::Retry {
            max_retries: 2,
            backoff_ms: 1,
            backoff_multiplier: 1.0,
        });

        // Fails all attempts (need 4 failures to exhaust 1 attempt + 2 retries + 1 more)
        let executor = FailNTimesMockExecutor::new(10);

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert!(result.step_results["flaky"].error.is_some());
    }

    #[tokio::test]
    async fn dag_step_failure_with_skip() {
        let mut steps = vec![
            dag_step("a", &[]),
            dag_step("b", &["a"]),
            dag_step("c", &["b"]),
        ];
        steps[1].on_error = OnError::SkipStep;

        let executor = MockExecutor::new()
            .with_response("a", "ok")
            .with_failure("b", "b failed")
            .with_response("c", "c_ran");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        // b failed but was skipped, c should still run
        // since b is in step_results (as skipped/failed), c's deps are met
        assert!(result.step_results.contains_key("c"));
    }

    #[tokio::test]
    async fn dag_step_failure_cascades() {
        let steps = vec![
            dag_step("a", &[]),
            dag_step("b", &["a"]),
            dag_step("c", &["b"]),
        ];

        let executor = MockExecutor::new()
            .with_response("a", "ok")
            .with_failure("b", "b failed")
            .with_response("c", "should_not_run");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert!(result.step_results["b"].error.is_some());
        // c should be cancelled since b failed (FailWorkflow is default)
        assert_eq!(result.step_results["c"].status, StepStatus::Cancelled);
    }

    #[tokio::test]
    async fn dag_empty_workflow() {
        let executor = MockExecutor::new();
        let result = execute_dag("test", &[], "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Failed);
        assert!(!result.validation_errors.is_empty());
    }

    #[tokio::test]
    async fn dag_single_step() {
        let steps = vec![dag_step("only", &[])];
        let executor = MockExecutor::new().with_response("only", "done");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert_eq!(result.step_results.len(), 1);
        assert_eq!(result.step_results["only"].response, "done");
    }

    #[tokio::test]
    async fn dag_all_steps_fail() {
        let steps = vec![dag_step("a", &[]), dag_step("b", &[])];

        let executor = MockExecutor::new()
            .with_failure("a", "a failed")
            .with_failure("b", "b failed");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Failed);
        assert!(!result.dead_letters.is_empty());
    }

    #[tokio::test]
    async fn dag_partial_completion() {
        let steps = vec![
            dag_step("good", &[]),
            dag_step("bad", &[]),
        ];

        let executor = MockExecutor::new()
            .with_response("good", "ok")
            .with_failure("bad", "nope");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::PartiallyCompleted);
    }

    #[tokio::test]
    async fn dag_validation_rejects_cycle() {
        let steps = vec![dag_step("a", &["b"]), dag_step("b", &["a"])];
        let executor = MockExecutor::new();
        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Failed);
        assert!(!result.validation_errors.is_empty());
    }

    #[tokio::test]
    async fn dag_all_steps_skipped() {
        let mut steps = vec![dag_step("a", &[]), dag_step("b", &[])];
        steps[0].condition = Some(Condition::Expression("false".to_string()));
        steps[1].condition = Some(Condition::Expression("false".to_string()));

        let executor = MockExecutor::new();
        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        // All skipped = no failures, no successes -> Completed
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert_eq!(result.step_results["a"].status, StepStatus::Skipped);
        assert_eq!(result.step_results["b"].status, StepStatus::Skipped);
    }

    // ---- DAG variable substitution tests ----

    #[test]
    fn dag_variables_step_output() {
        let mut results = HashMap::new();
        results.insert(
            "analyze".to_string(),
            StepResult {
                step_name: "analyze".to_string(),
                response: "found 3 bugs".to_string(),
                tokens_used: 100,
                duration_ms: 500,
                error: None,
                status: StepStatus::Completed,
                started_at: None,
                completed_at: None,
            },
        );

        let expanded = expand_dag_variables(
            "Result: {{analyze.output}}",
            "input",
            "next",
            &results,
            None,
        );
        assert_eq!(expanded, "Result: found 3 bugs");
    }

    #[test]
    fn dag_variables_step_status() {
        let mut results = HashMap::new();
        results.insert(
            "build".to_string(),
            StepResult {
                step_name: "build".to_string(),
                response: "ok".to_string(),
                tokens_used: 50,
                duration_ms: 300,
                error: None,
                status: StepStatus::Completed,
                started_at: None,
                completed_at: None,
            },
        );

        let expanded = expand_dag_variables(
            "Build status: {{build.status}}",
            "input",
            "deploy",
            &results,
            None,
        );
        assert_eq!(expanded, "Build status: completed");
    }

    #[test]
    fn dag_variables_step_duration() {
        let mut results = HashMap::new();
        results.insert(
            "fetch".to_string(),
            StepResult {
                step_name: "fetch".to_string(),
                response: "data".to_string(),
                tokens_used: 10,
                duration_ms: 1234,
                error: None,
                status: StepStatus::Completed,
                started_at: None,
                completed_at: None,
            },
        );

        let expanded = expand_dag_variables(
            "Fetch took {{fetch.duration_ms}}ms",
            "input",
            "next",
            &results,
            None,
        );
        assert_eq!(expanded, "Fetch took 1234ms");
    }

    #[test]
    fn dag_variables_loop_state() {
        let results = HashMap::new();
        let mut loop_state = LoopState::new();
        loop_state.index = 2;
        loop_state.item = Some("banana".to_string());

        let expanded = expand_dag_variables(
            "Item {{loop.index}}: {{loop.item}}",
            "input",
            "process",
            &results,
            Some(&loop_state),
        );
        assert_eq!(expanded, "Item 2: banana");
    }

    #[test]
    fn dag_variables_json_path() {
        let mut results = HashMap::new();
        results.insert(
            "api".to_string(),
            StepResult {
                step_name: "api".to_string(),
                response: r#"{"user": {"name": "Alice", "age": 30}}"#.to_string(),
                tokens_used: 10,
                duration_ms: 100,
                error: None,
                status: StepStatus::Completed,
                started_at: None,
                completed_at: None,
            },
        );

        let expanded = expand_dag_variables(
            "Name: {{api.output.user.name}}",
            "input",
            "next",
            &results,
            None,
        );
        assert_eq!(expanded, "Name: Alice");
    }

    #[test]
    fn dag_variables_transform_uppercase() {
        let mut results = HashMap::new();
        results.insert(
            "greet".to_string(),
            StepResult {
                step_name: "greet".to_string(),
                response: "hello world".to_string(),
                tokens_used: 10,
                duration_ms: 50,
                error: None,
                status: StepStatus::Completed,
                started_at: None,
                completed_at: None,
            },
        );

        let expanded = expand_dag_variables(
            "{{greet.output | uppercase}}",
            "input",
            "next",
            &results,
            None,
        );
        assert_eq!(expanded, "HELLO WORLD");
    }

    #[test]
    fn dag_variables_transform_lowercase() {
        let mut results = HashMap::new();
        results.insert(
            "shout".to_string(),
            StepResult {
                step_name: "shout".to_string(),
                response: "LOUD NOISE".to_string(),
                tokens_used: 10,
                duration_ms: 50,
                error: None,
                status: StepStatus::Completed,
                started_at: None,
                completed_at: None,
            },
        );

        let expanded = expand_dag_variables(
            "{{shout.output | lowercase}}",
            "input",
            "next",
            &results,
            None,
        );
        assert_eq!(expanded, "loud noise");
    }

    #[test]
    fn dag_variables_transform_json_extract() {
        let mut results = HashMap::new();
        results.insert(
            "data".to_string(),
            StepResult {
                step_name: "data".to_string(),
                response: r#"{"key": "value123"}"#.to_string(),
                tokens_used: 10,
                duration_ms: 50,
                error: None,
                status: StepStatus::Completed,
                started_at: None,
                completed_at: None,
            },
        );

        let expanded = expand_dag_variables(
            "{{data.output | json_extract \"$.key\"}}",
            "input",
            "next",
            &results,
            None,
        );
        assert_eq!(expanded, "value123");
    }

    #[test]
    fn json_path_extract_simple() {
        let result = json_path_extract(r#"{"name": "Bob"}"#, "name");
        assert_eq!(result, "Bob");
    }

    #[test]
    fn json_path_extract_nested() {
        let result = json_path_extract(r#"{"a": {"b": {"c": 42}}}"#, "a.b.c");
        assert_eq!(result, "42");
    }

    #[test]
    fn json_path_extract_dollar_prefix() {
        let result = json_path_extract(r#"{"key": "val"}"#, "$.key");
        assert_eq!(result, "val");
    }

    #[test]
    fn json_path_extract_missing_key() {
        let result = json_path_extract(r#"{"key": "val"}"#, "missing");
        assert_eq!(result, "");
    }

    #[test]
    fn json_path_extract_invalid_json() {
        let result = json_path_extract("not json", "key");
        assert_eq!(result, "not json");
    }

    // ---- Step status tests ----

    #[test]
    fn step_status_display() {
        assert_eq!(StepStatus::Pending.to_string(), "pending");
        assert_eq!(StepStatus::Running.to_string(), "running");
        assert_eq!(StepStatus::Completed.to_string(), "completed");
        assert_eq!(StepStatus::Failed.to_string(), "failed");
        assert_eq!(StepStatus::Skipped.to_string(), "skipped");
        assert_eq!(StepStatus::Cancelled.to_string(), "cancelled");
    }

    // ---- On error variant tests ----

    #[test]
    fn on_error_fallback_serialization() {
        let on_error = OnError::Fallback {
            step: "backup".to_string(),
        };
        let json = serde_json::to_string(&on_error).expect("serialize");
        let deser: OnError = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(deser, OnError::Fallback { step } if step == "backup"));
    }

    #[test]
    fn on_error_catch_and_continue_serialization() {
        let on_error = OnError::CatchAndContinue {
            error_handler: "handler".to_string(),
        };
        let json = serde_json::to_string(&on_error).expect("serialize");
        let deser: OnError = serde_json::from_str(&json).expect("deserialize");
        assert!(
            matches!(deser, OnError::CatchAndContinue { error_handler } if error_handler == "handler")
        );
    }

    #[test]
    fn on_error_circuit_breaker_serialization() {
        let on_error = OnError::CircuitBreaker {
            max_failures: 5,
            cooldown_secs: 60,
        };
        let json = serde_json::to_string(&on_error).expect("serialize");
        let deser: OnError = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(
            deser,
            OnError::CircuitBreaker {
                max_failures: 5,
                cooldown_secs: 60
            }
        ));
    }

    // ---- Circuit breaker tests ----

    #[test]
    fn circuit_breaker_default_closed() {
        let cb = CircuitBreakerState::default();
        assert!(!cb.is_open(3, 60));
    }

    #[test]
    fn circuit_breaker_opens_after_max_failures() {
        let mut cb = CircuitBreakerState::default();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(cb.is_open(3, 60));
    }

    #[test]
    fn circuit_breaker_resets_on_success() {
        let mut cb = CircuitBreakerState::default();
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        assert!(!cb.is_open(3, 60));
        assert_eq!(cb.consecutive_failures, 0);
    }

    // ---- DAG workflow registration tests ----

    #[test]
    fn register_dag_workflow_valid() {
        let engine = WorkflowEngine::new();
        let wf = DagWorkflow {
            id: WorkflowId::new(),
            name: "test-dag".to_string(),
            steps: vec![dag_step("a", &[]), dag_step("b", &["a"])],
        };
        let result = engine.register_dag_workflow(wf);
        assert!(result.is_ok());
    }

    #[test]
    fn register_dag_workflow_with_cycle_fails() {
        let engine = WorkflowEngine::new();
        let wf = DagWorkflow {
            id: WorkflowId::new(),
            name: "bad-dag".to_string(),
            steps: vec![dag_step("a", &["b"]), dag_step("b", &["a"])],
        };
        let result = engine.register_dag_workflow(wf);
        assert!(result.is_err());
    }

    #[test]
    fn list_dag_workflows() {
        let engine = WorkflowEngine::new();
        let wf = DagWorkflow {
            id: WorkflowId::new(),
            name: "dag1".to_string(),
            steps: vec![dag_step("a", &[])],
        };
        engine.register_dag_workflow(wf).expect("should register");
        assert_eq!(engine.list_dag_workflows().len(), 1);
    }

    #[test]
    fn get_dag_workflow() {
        let engine = WorkflowEngine::new();
        let id = WorkflowId::new();
        let wf = DagWorkflow {
            id,
            name: "dag1".to_string(),
            steps: vec![dag_step("a", &[])],
        };
        engine.register_dag_workflow(wf).expect("should register");
        let fetched = engine.get_dag_workflow(&id).expect("should exist");
        assert_eq!(fetched.name, "dag1");
    }

    #[test]
    fn get_nonexistent_dag_workflow() {
        let engine = WorkflowEngine::new();
        assert!(engine.get_dag_workflow(&WorkflowId::new()).is_none());
    }

    // ---- Dead letter queue tests ----

    #[tokio::test]
    async fn dag_dead_letters_populated_on_failure() {
        let steps = vec![dag_step("a", &[])];
        let executor = MockExecutor::new().with_failure("a", "catastrophic failure");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert!(!result.dead_letters.is_empty());
        assert_eq!(result.dead_letters[0].step_name, "a");
        assert_eq!(result.dead_letters[0].error, "catastrophic failure");
    }

    // ---- Execution trace tests ----

    #[tokio::test]
    async fn dag_execution_trace_records_waves() {
        let steps = vec![
            dag_step("a", &[]),
            dag_step("b", &["a"]),
            dag_step("c", &["b"]),
        ];
        let executor = MockExecutor::new()
            .with_response("a", "ok")
            .with_response("b", "ok")
            .with_response("c", "ok");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        // 3 waves for a linear chain
        assert_eq!(result.execution_trace.len(), 3);
        assert_eq!(result.execution_trace[0].steps, vec!["a"]);
        assert_eq!(result.execution_trace[1].steps, vec!["b"]);
        assert_eq!(result.execution_trace[2].steps, vec!["c"]);
    }

    // ---- DagWorkflowStep helper tests ----

    #[test]
    fn dag_step_fallback_step_extraction() {
        let mut step = dag_step("test", &[]);
        assert!(step.fallback_step().is_none());

        step.on_error = OnError::Fallback {
            step: "backup".to_string(),
        };
        assert_eq!(step.fallback_step(), Some("backup".to_string()));

        step.on_error = OnError::CatchAndContinue {
            error_handler: "handler".to_string(),
        };
        assert_eq!(step.fallback_step(), Some("handler".to_string()));
    }

    // ---- Serialization tests for new types ----

    #[test]
    fn dag_workflow_serialization_roundtrip() {
        let wf = DagWorkflow {
            id: WorkflowId::new(),
            name: "test-dag".to_string(),
            steps: vec![
                dag_step("a", &[]),
                dag_step("b", &["a"]),
            ],
        };
        let json = serde_json::to_string(&wf).expect("serialize");
        let deser: DagWorkflow = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.name, "test-dag");
        assert_eq!(deser.steps.len(), 2);
    }

    #[test]
    fn dag_workflow_step_with_condition_serialization() {
        let mut step = dag_step("test", &["dep1"]);
        step.condition = Some(Condition::IfSuccess {
            step: "dep1".to_string(),
        });
        step.else_step = Some("fallback".to_string());
        let json = serde_json::to_string(&step).expect("serialize");
        let deser: DagWorkflowStep = serde_json::from_str(&json).expect("deserialize");
        assert!(deser.condition.is_some());
        assert_eq!(deser.else_step, Some("fallback".to_string()));
    }

    #[test]
    fn dead_letter_entry_serialization() {
        let entry = DeadLetterEntry {
            step_name: "failed_step".to_string(),
            error: "boom".to_string(),
            input: "test input".to_string(),
            failed_at: Utc::now(),
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        let deser: DeadLetterEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.step_name, "failed_step");
        assert_eq!(deser.error, "boom");
    }

    #[test]
    fn execution_trace_entry_serialization() {
        let entry = ExecutionTraceEntry {
            steps: vec!["a".to_string(), "b".to_string()],
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        let deser: ExecutionTraceEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.steps.len(), 2);
    }

    #[test]
    fn workflow_run_with_new_fields_serialization() {
        let run = WorkflowRun {
            id: WorkflowRunId::new(),
            workflow_id: WorkflowId::new(),
            status: WorkflowRunStatus::PartiallyCompleted,
            step_results: Vec::new(),
            started_at: Utc::now(),
            completed_at: None,
            dead_letters: vec![DeadLetterEntry {
                step_name: "x".to_string(),
                error: "err".to_string(),
                input: "in".to_string(),
                failed_at: Utc::now(),
            }],
            execution_trace: Vec::new(),
        };
        let json = serde_json::to_string(&run).expect("serialize");
        let deser: WorkflowRun = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.status, WorkflowRunStatus::PartiallyCompleted);
        assert_eq!(deser.dead_letters.len(), 1);
    }

    #[test]
    fn step_result_with_new_fields() {
        let sr = StepResult {
            step_name: "test".to_string(),
            response: "ok".to_string(),
            tokens_used: 10,
            duration_ms: 100,
            error: None,
            status: StepStatus::Completed,
            started_at: Some(Utc::now()),
            completed_at: Some(Utc::now()),
        };
        let json = serde_json::to_string(&sr).expect("serialize");
        let deser: StepResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.status, StepStatus::Completed);
        assert!(deser.started_at.is_some());
    }

    // ---- Fallback error handling test ----

    #[tokio::test]
    async fn dag_fallback_on_error() {
        let mut steps = vec![
            dag_step("main", &[]),
            dag_step("backup", &[]),
        ];
        steps[0].on_error = OnError::Fallback {
            step: "backup".to_string(),
        };

        let executor = MockExecutor::new()
            .with_failure("main", "main failed")
            .with_response("backup", "backup result");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        // The main step should have used backup's result
        // In our implementation, the step result gets the backup response
        let main_result = &result.step_results["main"];
        assert_eq!(main_result.response, "backup result");
    }

    #[tokio::test]
    async fn dag_catch_and_continue() {
        let mut steps = vec![
            dag_step("risky", &[]),
            dag_step("handler", &[]),
            dag_step("next", &["risky"]),
        ];
        steps[0].on_error = OnError::CatchAndContinue {
            error_handler: "handler".to_string(),
        };

        let executor = MockExecutor::new()
            .with_failure("risky", "oops")
            .with_response("handler", "handled")
            .with_response("next", "continued");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        // "next" should have run because CatchAndContinue removes the failure
        assert!(result.step_results.contains_key("next"));
    }

    // ---- Parallel execution proof tests ----

    /// A timed executor that records start/end times to prove concurrency.
    struct ConcurrencyProofExecutor {
        delay_ms: u64,
        /// Track (step_name, start_instant, end_instant) for each execution.
        timings: Arc<tokio::sync::Mutex<Vec<(String, Instant, Instant)>>>,
    }

    impl ConcurrencyProofExecutor {
        fn new(delay_ms: u64) -> Self {
            Self {
                delay_ms,
                timings: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            }
        }
    }

    #[async_trait::async_trait]
    impl StepExecutor for ConcurrencyProofExecutor {
        async fn execute(
            &self,
            step: &DagWorkflowStep,
            _input: &str,
            _step_results: &HashMap<String, StepResult>,
            _loop_state: Option<&LoopState>,
        ) -> Result<StepResult, String> {
            let start = Instant::now();
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            let end = Instant::now();

            self.timings
                .lock()
                .await
                .push((step.name.clone(), start, end));

            Ok(StepResult {
                step_name: step.name.clone(),
                response: format!("done-{}", step.name),
                tokens_used: 10,
                duration_ms: self.delay_ms,
                error: None,
                status: StepStatus::Completed,
                started_at: Some(Utc::now()),
                completed_at: Some(Utc::now()),
            })
        }
    }

    /// Prove 3 independent steps with 50ms sleep each complete in ~50-70ms (not 150ms).
    #[tokio::test]
    async fn dag_three_independent_steps_parallel_timing() {
        let steps = vec![
            dag_step("x", &[]),
            dag_step("y", &[]),
            dag_step("z", &[]),
        ];
        let executor = ConcurrencyProofExecutor::new(50);
        let timings = Arc::clone(&executor.timings);

        let start = Instant::now();
        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        let elapsed = start.elapsed();

        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert_eq!(result.step_results.len(), 3);
        // Parallel: should finish in ~50ms, not 150ms
        assert!(
            elapsed.as_millis() < 100,
            "3 independent 50ms steps took {}ms, should be ~50ms for parallel execution",
            elapsed.as_millis()
        );

        // Verify that the steps overlapped in time
        let recorded = timings.lock().await;
        assert_eq!(recorded.len(), 3);
        // All should have started within a few ms of each other
        let starts: Vec<_> = recorded.iter().map(|(_, s, _)| *s).collect();
        let earliest = starts.iter().min().copied().expect("should have starts");
        for s in &starts {
            let diff = s.duration_since(earliest).as_millis();
            assert!(
                diff < 20,
                "start time spread {}ms too large for parallel execution",
                diff
            );
        }
    }

    /// Fan-out: step A -> steps B,C,D in parallel -> step E waits for all.
    #[tokio::test]
    async fn dag_fan_out_fan_in_timing() {
        let steps = vec![
            dag_step("a", &[]),
            dag_step("b", &["a"]),
            dag_step("c", &["a"]),
            dag_step("d", &["a"]),
            dag_step("e", &["b", "c", "d"]),
        ];
        let executor = TimedMockExecutor { delay_ms: 30 };

        let start = Instant::now();
        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        let elapsed = start.elapsed();

        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert_eq!(result.step_results.len(), 5);

        // 3 waves: A (30ms) + B,C,D parallel (30ms) + E (30ms) = ~90ms
        // Sequential would be 5*30 = 150ms
        assert!(
            elapsed.as_millis() < 130,
            "fan-out/fan-in took {}ms, expected ~90ms",
            elapsed.as_millis()
        );

        // Verify execution trace shows 3 waves
        assert_eq!(result.execution_trace.len(), 3);
        // Wave 2 should have B, C, D
        let wave2 = &result.execution_trace[1].steps;
        assert_eq!(wave2.len(), 3);
    }

    /// Fan-in: multiple parallel roots feed into one join step.
    #[tokio::test]
    async fn dag_fan_in_parallel_roots() {
        let steps = vec![
            dag_step("r1", &[]),
            dag_step("r2", &[]),
            dag_step("r3", &[]),
            dag_step("join", &["r1", "r2", "r3"]),
        ];
        let executor = MockExecutor::new()
            .with_response("r1", "out1")
            .with_response("r2", "out2")
            .with_response("r3", "out3")
            .with_response("join", "merged");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert_eq!(result.step_results["join"].response, "merged");
        // r1, r2, r3 in wave 1, join in wave 2
        assert_eq!(result.execution_trace.len(), 2);
        assert_eq!(result.execution_trace[0].steps.len(), 3);
    }

    /// Diamond dependency: A -> B,C -> D (D depends on both B and C).
    #[tokio::test]
    async fn dag_diamond_dependency_parallel() {
        let steps = vec![
            dag_step("a", &[]),
            dag_step("b", &["a"]),
            dag_step("c", &["a"]),
            dag_step("d", &["b", "c"]),
        ];
        let executor = TimedMockExecutor { delay_ms: 30 };

        let start = Instant::now();
        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        let elapsed = start.elapsed();

        assert_eq!(result.status, WorkflowRunStatus::Completed);
        // 3 waves: A, B+C parallel, D
        assert_eq!(result.execution_trace.len(), 3);
        // B and C should be in the same wave
        let wave2 = &result.execution_trace[1].steps;
        assert!(wave2.contains(&"b".to_string()));
        assert!(wave2.contains(&"c".to_string()));
        // Total should be ~90ms (3 waves * 30ms), not 120ms (4 sequential)
        assert!(
            elapsed.as_millis() < 120,
            "diamond took {}ms, expected ~90ms",
            elapsed.as_millis()
        );
    }

    /// Conditional skipping in a DAG.
    #[tokio::test]
    async fn dag_conditional_skip_in_dag() {
        let mut steps = vec![
            dag_step("check", &[]),
            dag_step("true_branch", &["check"]),
            dag_step("false_branch", &["check"]),
        ];
        // true_branch runs only if check succeeds (it will)
        steps[1].condition = Some(Condition::IfSuccess {
            step: "check".to_string(),
        });
        // false_branch runs only if check fails (it won't)
        steps[2].condition = Some(Condition::IfFailure {
            step: "check".to_string(),
        });

        let executor = MockExecutor::new()
            .with_response("check", "all good")
            .with_response("true_branch", "ran")
            .with_response("false_branch", "should_not_run");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.step_results["true_branch"].status, StepStatus::Completed);
        assert_eq!(result.step_results["false_branch"].status, StepStatus::Skipped);
    }

    /// Loop execution within a DAG step (ForEach).
    #[tokio::test]
    async fn dag_loop_foreach_within_dag() {
        let mut steps = vec![
            dag_step("data", &[]),
            dag_step("process", &["data"]),
            dag_step("summary", &["process"]),
        ];
        steps[1].loop_config = Some(LoopConfig::ForEach {
            source_step: "data".to_string(),
            max_iterations: 10,
        });
        steps[1].prompt_template = "process: {{loop.item}}".to_string();

        let executor = MockExecutor::new()
            .with_response("data", r#"["red", "green", "blue"]"#)
            .with_response("summary", "done");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        let process_out = &result.step_results["process"].response;
        // Should contain output from all 3 loop iterations
        assert!(process_out.contains("process: red"));
        assert!(process_out.contains("process: green"));
        assert!(process_out.contains("process: blue"));
    }

    /// Partial failure: one parallel branch fails, others succeed.
    #[tokio::test]
    async fn dag_partial_failure_parallel_branches() {
        let steps = vec![
            dag_step("root", &[]),
            dag_step("ok_branch", &["root"]),
            dag_step("fail_branch", &["root"]),
            dag_step("ok_branch2", &["root"]),
        ];

        let executor = MockExecutor::new()
            .with_response("root", "start")
            .with_response("ok_branch", "success1")
            .with_failure("fail_branch", "branch failed")
            .with_response("ok_branch2", "success2");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::PartiallyCompleted);
        assert_eq!(result.step_results["ok_branch"].status, StepStatus::Completed);
        assert_eq!(result.step_results["ok_branch2"].status, StepStatus::Completed);
        assert!(result.step_results["fail_branch"].error.is_some());
    }

    /// Fallback step execution on failure.
    #[tokio::test]
    async fn dag_fallback_step_runs_on_failure() {
        let mut steps = vec![
            dag_step("primary", &[]),
            dag_step("fallback_handler", &[]),
            dag_step("downstream", &["primary"]),
        ];
        steps[0].on_error = OnError::Fallback {
            step: "fallback_handler".to_string(),
        };

        let executor = MockExecutor::new()
            .with_failure("primary", "primary broke")
            .with_response("fallback_handler", "recovered via fallback")
            .with_response("downstream", "downstream ran");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        // primary should have the fallback result
        let primary_result = &result.step_results["primary"];
        assert_eq!(primary_result.response, "recovered via fallback");
        // downstream should have run since fallback recovered
        assert!(result.step_results.contains_key("downstream"));
    }

    /// Circuit breaker triggering after N failures.
    #[tokio::test]
    async fn dag_circuit_breaker_triggers() {
        let mut steps = vec![dag_step("cb_step", &[])];
        steps[0].on_error = OnError::CircuitBreaker {
            max_failures: 2,
            cooldown_secs: 300,
        };

        // First run: fail twice to trip the breaker
        let executor1 = MockExecutor::new().with_failure("cb_step", "fail1");
        let result1 = execute_dag("test", &steps, "input", Arc::new(executor1)).await;
        assert!(result1.step_results["cb_step"].error.is_some());

        // The circuit breaker state is per-run, so we test within a single run
        // with a step that has CircuitBreaker and fails. The breaker opens internally
        // after max_failures. Let's verify the circuit breaker state logic directly.
        let mut cb = CircuitBreakerState::default();
        cb.record_failure();
        assert!(!cb.is_open(2, 300), "should not be open after 1 failure");
        cb.record_failure();
        assert!(cb.is_open(2, 300), "should be open after 2 failures");
        // After cooldown, it should close — but since cooldown is 300s, it's still open
        assert!(cb.is_open(2, 300));
    }

    /// Variable substitution works across parallel branches.
    #[tokio::test]
    async fn dag_variable_substitution_across_parallel_branches() {
        let mut steps = vec![
            dag_step("source_a", &[]),
            dag_step("source_b", &[]),
            dag_step("consumer", &["source_a", "source_b"]),
        ];
        steps[2].prompt_template =
            "A={{source_a.output}}, B={{source_b.output}}".to_string();

        let executor = MockExecutor::new()
            .with_response("source_a", "value_from_a")
            .with_response("source_b", "value_from_b");
        // consumer doesn't have a fixed response, so it will use the expanded prompt

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        let consumer_out = &result.step_results["consumer"].response;
        assert!(
            consumer_out.contains("value_from_a"),
            "consumer should see source_a output, got: {consumer_out}"
        );
        assert!(
            consumer_out.contains("value_from_b"),
            "consumer should see source_b output, got: {consumer_out}"
        );
    }

    /// Wide parallel fan-out with timing proof.
    #[tokio::test]
    async fn dag_wide_parallel_fan_out_timing() {
        // 10 independent steps each taking 30ms
        let steps: Vec<DagWorkflowStep> = (0..10)
            .map(|i| dag_step(&format!("s{i}"), &[]))
            .collect();
        let executor = TimedMockExecutor { delay_ms: 30 };

        let start = Instant::now();
        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        let elapsed = start.elapsed();

        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert_eq!(result.step_results.len(), 10);
        // All 10 should run in one wave (~30ms), not sequentially (~300ms)
        assert!(
            elapsed.as_millis() < 80,
            "10 parallel 30ms steps took {}ms, expected ~30ms",
            elapsed.as_millis()
        );
        assert_eq!(result.execution_trace.len(), 1);
        assert_eq!(result.execution_trace[0].steps.len(), 10);
    }

    /// While loop with condition that eventually terminates.
    #[tokio::test]
    async fn dag_while_loop_with_condition() {
        let mut steps = vec![dag_step("looper", &[])];
        steps[0].loop_config = Some(LoopConfig::While {
            condition: Condition::Expression("true".to_string()),
            max_iterations: 3,
        });

        let executor = MockExecutor::new().with_response("looper", "iteration");

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        let output = &result.step_results["looper"].response;
        // Should have 3 iterations
        let lines: Vec<&str> = output.split('\n').collect();
        assert_eq!(lines.len(), 3);
    }

    /// Retry loop succeeds on second attempt.
    #[tokio::test]
    async fn dag_retry_succeeds_on_retry() {
        let mut steps = vec![dag_step("retry_step", &[])];
        steps[0].loop_config = Some(LoopConfig::Retry {
            max_retries: 2,
            backoff_ms: 1,
            backoff_multiplier: 1.0,
        });

        let executor = FailNTimesMockExecutor::new(1);

        let result = execute_dag("test", &steps, "input", Arc::new(executor)).await;
        assert_eq!(result.status, WorkflowRunStatus::Completed);
        assert!(result.step_results["retry_step"].error.is_none());
        assert!(result.step_results["retry_step"]
            .response
            .contains("success on attempt 2"));
    }
}
