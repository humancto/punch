//! Multi-step agent workflow engine.
//!
//! The [`WorkflowEngine`] allows registering named workflows composed of
//! sequential steps. Each step invokes a fighter with a prompt template that
//! supports variable substitution (`{{input}}`, `{{step_name}}`,
//! `{{previous_output}}`, and `{{step_name_ref}}`).

use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{error, info, instrument, warn};
use uuid::Uuid;

use punch_memory::MemorySubstrate;
use punch_runtime::{FighterLoopParams, LlmDriver, run_fighter_loop, tools_for_capabilities};
use punch_types::{FighterId, FighterManifest, ModelConfig, PunchError, PunchResult, WeightClass};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

/// A single step within a workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    /// Human-readable name for this step.
    pub name: String,
    /// The fighter name to use for this step.
    pub fighter_name: String,
    /// Prompt template with `{{input}}`, `{{step_name}}`, `{{previous_output}}`,
    /// and `{{step_ref}}` variables.
    pub prompt_template: String,
    /// Maximum time in seconds for this step (default 120).
    pub timeout_secs: Option<u64>,
    /// Error handling strategy.
    #[serde(default)]
    pub on_error: OnError,
}

/// A workflow definition composed of sequential steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    /// Unique identifier.
    pub id: WorkflowId,
    /// Human-readable name.
    pub name: String,
    /// Ordered steps to execute.
    pub steps: Vec<WorkflowStep>,
}

/// Status of a workflow run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

impl std::fmt::Display for WorkflowRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
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
}

// ---------------------------------------------------------------------------
// Variable substitution
// ---------------------------------------------------------------------------

/// Replace template variables in a prompt string.
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

// ---------------------------------------------------------------------------
// WorkflowEngine
// ---------------------------------------------------------------------------

/// Engine for registering and executing multi-step agent workflows.
pub struct WorkflowEngine {
    /// Registered workflow definitions.
    workflows: DashMap<WorkflowId, Workflow>,
    /// Workflow execution runs.
    runs: DashMap<WorkflowRunId, WorkflowRun>,
}

impl WorkflowEngine {
    /// Create a new workflow engine.
    pub fn new() -> Self {
        Self {
            workflows: DashMap::new(),
            runs: DashMap::new(),
        }
    }

    /// Register a workflow definition and return its ID.
    pub fn register_workflow(&self, workflow: Workflow) -> WorkflowId {
        let id = workflow.id;
        info!(workflow_id = %id, name = %workflow.name, "workflow registered");
        self.workflows.insert(id, workflow);
        id
    }

    /// Execute a workflow with the given input string.
    ///
    /// Steps are executed sequentially. The prompt template for each step can
    /// reference `{{input}}` (the original input or previous step's output),
    /// `{{previous_output}}` (alias for `{{input}}`),
    /// `{{step_name}}` (the name of the current step),
    /// `{{step_N}}` (output of step N), and `{{some_step_name}}` (output by name).
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
                            };
                            step_results.push(skip_result);
                            // current_input stays the same for the next step
                            continue;
                        }
                        OnError::RetryOnce => {
                            warn!(step = %step.name, error = %error_msg, "step failed, retrying once");
                            // Retry the step once.
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

    /// List all registered workflows.
    pub fn list_workflows(&self) -> Vec<Workflow> {
        self.workflows.iter().map(|w| w.value().clone()).collect()
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

    /// Get a workflow by its ID.
    pub fn get_workflow(&self, id: &WorkflowId) -> Option<Workflow> {
        self.workflows.get(id).map(|w| w.clone())
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

        let fetched = engine.get_workflow(&id).unwrap();
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
            },
            StepResult {
                step_name: "review".to_string(),
                response: "review result".to_string(),
                tokens_used: 80,
                duration_ms: 400,
                error: None,
            },
        ];

        // By step number (1-indexed)
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

        // By step name
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
        // Should be a valid UUID.
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

        // No runs initially.
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
        let json = serde_json::to_string(&step).unwrap();
        let deserialized: WorkflowStep = serde_json::from_str(&json).unwrap();
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
        let json = serde_json::to_string(&workflow).unwrap();
        let deserialized: Workflow = serde_json::from_str(&json).unwrap();
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
        };
        assert!(sr.error.is_some());
        assert_eq!(sr.error.unwrap(), "timeout");
    }

    #[test]
    fn variable_substitution_step_ref_by_number_out_of_range() {
        // {{step_5}} when we only have 2 steps should remain unreplaced.
        let step_results = vec![
            StepResult {
                step_name: "a".to_string(),
                response: "r1".to_string(),
                tokens_used: 0,
                duration_ms: 0,
                error: None,
            },
            StepResult {
                step_name: "b".to_string(),
                response: "r2".to_string(),
                tokens_used: 0,
                duration_ms: 0,
                error: None,
            },
        ];
        let result = expand_variables("{{step_5}}", "input", "step", &step_results);
        assert_eq!(result, "{{step_5}}");
    }
}
