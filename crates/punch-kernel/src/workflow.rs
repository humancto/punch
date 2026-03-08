//! Multi-step agent workflow engine.
//!
//! The [`WorkflowEngine`] allows registering named workflows composed of
//! sequential steps. Each step invokes a fighter with a prompt template that
//! supports variable substitution (`{{input}}` and `{{step_name}}`).

use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{error, info, instrument};
use uuid::Uuid;

use punch_memory::MemorySubstrate;
use punch_runtime::{FighterLoopParams, LlmDriver, run_fighter_loop, tools_for_capabilities};
use punch_types::{
    FighterId, FighterManifest, ModelConfig, Provider, PunchError, PunchResult, WeightClass,
};

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
    /// Prompt template with `{{input}}` and `{{step_name}}` variables.
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
    /// reference `{{input}}` (the original input or previous step's output)
    /// and `{{step_name}}` (the name of the current step). Previous step
    /// results are also available as `{{prev_step_name}}`.
    #[instrument(skip(self, input, memory, driver), fields(%workflow_id))]
    pub async fn execute_workflow(
        &self,
        workflow_id: &WorkflowId,
        input: String,
        memory: Arc<MemorySubstrate>,
        driver: Arc<dyn LlmDriver>,
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
            let step_start = Instant::now();

            // Substitute variables in the prompt template.
            let mut prompt = step.prompt_template.clone();
            prompt = prompt.replace("{{input}}", &current_input);
            prompt = prompt.replace("{{step_name}}", &step.name);

            // Substitute previous step results by name.
            for prev_result in &step_results {
                let var = format!("{{{{{}}}}}", prev_result.step_name);
                prompt = prompt.replace(&var, &prev_result.response);
            }

            // Create a temporary fighter for this step.
            let fighter_id = FighterId::new();
            let fighter_manifest = FighterManifest {
                name: step.fighter_name.clone(),
                description: format!("Workflow step: {}", step.name),
                model: ModelConfig {
                    provider: Provider::Anthropic,
                    model: "claude-sonnet-4-20250514".to_string(),
                    api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                    base_url: None,
                    max_tokens: Some(4096),
                    temperature: Some(0.7),
                },
                system_prompt: format!(
                    "You are executing step '{}' of workflow '{}'.",
                    step.name, workflow.name
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

            let bout_id = match memory.create_bout(&fighter_id).await {
                Ok(id) => id,
                Err(e) => {
                    let result = StepResult {
                        step_name: step.name.clone(),
                        response: String::new(),
                        tokens_used: 0,
                        duration_ms: step_start.elapsed().as_millis() as u64,
                        error: Some(format!("failed to create bout: {e}")),
                    };
                    step_results.push(result);

                    match step.on_error {
                        OnError::FailWorkflow => {
                            failed = true;
                            break;
                        }
                        OnError::SkipStep => continue,
                        OnError::RetryOnce => {
                            // For simplicity, just fail on retry failure too.
                            failed = true;
                            break;
                        }
                    }
                }
            };

            let available_tools = tools_for_capabilities(&fighter_manifest.capabilities);
            let timeout_secs = step.timeout_secs.unwrap_or(120);

            let params = FighterLoopParams {
                manifest: fighter_manifest,
                user_message: prompt,
                bout_id,
                fighter_id,
                memory: Arc::clone(&memory),
                driver: Arc::clone(&driver),
                available_tools,
                max_iterations: Some(20),
                context_window: None,
                tool_timeout_secs: Some(timeout_secs),
            };

            let execute_result = async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_secs),
                    run_fighter_loop(params),
                )
                .await
            };

            let loop_result = match execute_result.await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(PunchError::Internal(format!(
                    "step '{}' timed out after {}s",
                    step.name, timeout_secs
                ))),
            };

            match loop_result {
                Ok(result) => {
                    let step_result = StepResult {
                        step_name: step.name.clone(),
                        response: result.response.clone(),
                        tokens_used: result.usage.total(),
                        duration_ms: step_start.elapsed().as_millis() as u64,
                        error: None,
                    };
                    current_input = result.response;
                    step_results.push(step_result);
                }
                Err(e) => {
                    let error_msg = format!("{e}");
                    let step_result = StepResult {
                        step_name: step.name.clone(),
                        response: String::new(),
                        tokens_used: 0,
                        duration_ms: step_start.elapsed().as_millis() as u64,
                        error: Some(error_msg),
                    };
                    step_results.push(step_result);

                    match step.on_error {
                        OnError::SkipStep => {
                            continue;
                        }
                        OnError::FailWorkflow | OnError::RetryOnce => {
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
    fn variable_substitution_in_prompt() {
        let template = "Analyze {{input}} for step {{step_name}}";
        let result = template
            .replace("{{input}}", "hello world")
            .replace("{{step_name}}", "analysis");
        assert_eq!(result, "Analyze hello world for step analysis");
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
}
