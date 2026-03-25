//! End-to-end workflow tests covering registration, execution, sequential
//! pipelines, conditions, ForEach loops, failure handling, and variable
//! substitution.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use punch_kernel::workflow_conditions::{Condition, evaluate_condition};
use punch_kernel::workflow_loops::{LoopConfig, LoopState, calculate_backoff, parse_foreach_items};
use punch_kernel::{
    CircuitBreakerState, DagWorkflowStep, OnError, Ring, StepExecutor, StepResult, StepStatus,
    Workflow, WorkflowEngine, WorkflowId, WorkflowRunStatus, WorkflowStep, execute_dag,
    expand_dag_variables,
};
use punch_memory::MemorySubstrate;
use punch_runtime::{CompletionRequest, CompletionResponse, LlmDriver, StopReason, TokenUsage};
use punch_types::{ModelConfig, Provider, PunchConfig, PunchResult};

// ---------------------------------------------------------------------------
// Mock LLM / Step Executor
// ---------------------------------------------------------------------------

struct MockLlmDriver {
    call_count: AtomicU64,
}

impl MockLlmDriver {
    fn new() -> Self {
        Self {
            call_count: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl LlmDriver for MockLlmDriver {
    async fn complete(&self, request: CompletionRequest) -> PunchResult<CompletionResponse> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        let user_content = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == punch_types::Role::User)
            .map(|m| m.content.clone())
            .unwrap_or_default();

        Ok(CompletionResponse {
            message: punch_types::Message {
                role: punch_types::Role::Assistant,
                content: format!("[mock-{}] {}", count, user_content),
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
                timestamp: chrono::Utc::now(),
            },
            usage: TokenUsage {
                input_tokens: 50,
                output_tokens: 25,
            },
            stop_reason: StopReason::EndTurn,
        })
    }
}

struct FailingLlmDriver;

#[async_trait]
impl LlmDriver for FailingLlmDriver {
    async fn complete(&self, _request: CompletionRequest) -> PunchResult<CompletionResponse> {
        Err(punch_types::PunchError::Provider {
            provider: "mock".to_string(),
            message: "intentional failure".to_string(),
        })
    }
}

/// A mock step executor for DAG tests that echoes the expanded prompt.
struct EchoStepExecutor;

#[async_trait]
impl StepExecutor for EchoStepExecutor {
    async fn execute(
        &self,
        step: &DagWorkflowStep,
        input: &str,
        step_results: &HashMap<String, StepResult>,
        loop_state: Option<&LoopState>,
    ) -> Result<StepResult, String> {
        let expanded = expand_dag_variables(
            &step.prompt_template,
            input,
            &step.name,
            step_results,
            loop_state,
        );
        Ok(StepResult {
            step_name: step.name.clone(),
            response: expanded,
            tokens_used: 10,
            duration_ms: 5,
            error: None,
            status: StepStatus::Completed,
            started_at: Some(chrono::Utc::now()),
            completed_at: Some(chrono::Utc::now()),
        })
    }
}

/// A step executor that fails for a specific step name.
struct FailingStepExecutor {
    fail_step: String,
}

#[async_trait]
impl StepExecutor for FailingStepExecutor {
    async fn execute(
        &self,
        step: &DagWorkflowStep,
        input: &str,
        step_results: &HashMap<String, StepResult>,
        loop_state: Option<&LoopState>,
    ) -> Result<StepResult, String> {
        if step.name == self.fail_step {
            Err("intentional step failure".to_string())
        } else {
            let expanded = expand_dag_variables(
                &step.prompt_template,
                input,
                &step.name,
                step_results,
                loop_state,
            );
            Ok(StepResult {
                step_name: step.name.clone(),
                response: expanded,
                tokens_used: 10,
                duration_ms: 5,
                error: None,
                status: StepStatus::Completed,
                started_at: Some(chrono::Utc::now()),
                completed_at: Some(chrono::Utc::now()),
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_config() -> PunchConfig {
    PunchConfig {
        api_listen: "127.0.0.1:0".to_string(),
        api_key: String::new(),
        rate_limit_rpm: 60,
        default_model: ModelConfig {
            provider: Provider::Ollama,
            model: "test-model".to_string(),
            api_key_env: None,
            base_url: Some("http://localhost:11434".to_string()),
            max_tokens: Some(4096),
            temperature: Some(0.7),
        },
        memory: punch_types::config::MemoryConfig {
            db_path: ":memory:".to_string(),
            knowledge_graph_enabled: false,
            max_entries: None,
        },
        tunnel: None,
        channels: Default::default(),
        mcp_servers: Default::default(),
        model_routing: Default::default(),
    }
}

fn create_ring(driver: Arc<dyn LlmDriver>) -> Arc<Ring> {
    let config = test_config();
    let memory = Arc::new(
        MemorySubstrate::new(std::path::Path::new(":memory:")).expect("memory should init"),
    );
    Arc::new(Ring::new(config, memory, driver))
}

// ---------------------------------------------------------------------------
// Tests: Sequential workflows via Ring
// ---------------------------------------------------------------------------

/// Register a workflow, execute, and verify completed status.
#[tokio::test]
async fn test_workflow_register_execute_completed() {
    let driver = Arc::new(MockLlmDriver::new());
    let ring = create_ring(Arc::clone(&driver) as Arc<dyn LlmDriver>);

    let workflow = Workflow {
        id: WorkflowId::new(),
        name: "simple-test".to_string(),
        steps: vec![WorkflowStep {
            name: "only_step".to_string(),
            fighter_name: "Fighter".to_string(),
            prompt_template: "Process: {{input}}".to_string(),
            timeout_secs: Some(30),
            on_error: OnError::FailWorkflow,
        }],
    };

    let wf_id = ring.register_workflow(workflow);
    let run_id = ring
        .execute_workflow(&wf_id, "test data".to_string())
        .await
        .expect("workflow should execute");

    let run = ring.workflow_engine().get_run(&run_id).unwrap();
    assert_eq!(run.status, WorkflowRunStatus::Completed);
    assert_eq!(run.step_results.len(), 1);
    assert_eq!(run.step_results[0].step_name, "only_step");
    assert!(run.step_results[0].tokens_used > 0);

    ring.shutdown();
}

/// Sequential 3-step workflow verifies all steps ran in order.
#[tokio::test]
async fn test_workflow_three_steps_sequential() {
    let driver = Arc::new(MockLlmDriver::new());
    let ring = create_ring(Arc::clone(&driver) as Arc<dyn LlmDriver>);

    let workflow = Workflow {
        id: WorkflowId::new(),
        name: "three-step".to_string(),
        steps: vec![
            WorkflowStep {
                name: "step1".to_string(),
                fighter_name: "F1".to_string(),
                prompt_template: "Step1: {{input}}".to_string(),
                timeout_secs: Some(30),
                on_error: OnError::FailWorkflow,
            },
            WorkflowStep {
                name: "step2".to_string(),
                fighter_name: "F2".to_string(),
                prompt_template: "Step2: {{input}}".to_string(),
                timeout_secs: Some(30),
                on_error: OnError::FailWorkflow,
            },
            WorkflowStep {
                name: "step3".to_string(),
                fighter_name: "F3".to_string(),
                prompt_template: "Step3: {{input}}".to_string(),
                timeout_secs: Some(30),
                on_error: OnError::FailWorkflow,
            },
        ],
    };

    let wf_id = ring.register_workflow(workflow);
    let run_id = ring
        .execute_workflow(&wf_id, "initial".to_string())
        .await
        .unwrap();

    let run = ring.workflow_engine().get_run(&run_id).unwrap();
    assert_eq!(run.status, WorkflowRunStatus::Completed);
    assert_eq!(run.step_results.len(), 3);
    assert_eq!(run.step_results[0].step_name, "step1");
    assert_eq!(run.step_results[1].step_name, "step2");
    assert_eq!(run.step_results[2].step_name, "step3");

    // Step 2 should contain step 1's output (chaining).
    assert!(
        run.step_results[1].response.contains("mock-0"),
        "step2 should reference step1 output"
    );

    ring.shutdown();
}

/// Workflow failure verifies error handling and status.
#[tokio::test]
async fn test_workflow_failure_status() {
    let driver: Arc<dyn LlmDriver> = Arc::new(FailingLlmDriver);
    let ring = create_ring(driver);

    let workflow = Workflow {
        id: WorkflowId::new(),
        name: "fail-test".to_string(),
        steps: vec![WorkflowStep {
            name: "failing".to_string(),
            fighter_name: "F".to_string(),
            prompt_template: "{{input}}".to_string(),
            timeout_secs: Some(10),
            on_error: OnError::FailWorkflow,
        }],
    };

    let wf_id = ring.register_workflow(workflow);
    let run_id = ring
        .execute_workflow(&wf_id, "test".to_string())
        .await
        .unwrap();

    let run = ring.workflow_engine().get_run(&run_id).unwrap();
    assert_eq!(run.status, WorkflowRunStatus::Failed);
    assert!(run.step_results[0].error.is_some());

    ring.shutdown();
}

// ---------------------------------------------------------------------------
// Tests: DAG execution via execute_dag
// ---------------------------------------------------------------------------

/// Parallel fan-out workflow with two independent steps.
#[tokio::test]
async fn test_dag_parallel_fan_out() {
    let executor: Arc<dyn StepExecutor> = Arc::new(EchoStepExecutor);

    let steps = vec![
        DagWorkflowStep {
            name: "branch_a".to_string(),
            fighter_name: "FA".to_string(),
            prompt_template: "BranchA: {{input}}".to_string(),
            timeout_secs: Some(30),
            on_error: OnError::FailWorkflow,
            depends_on: vec![],
            condition: None,
            else_step: None,
            loop_config: None,
        },
        DagWorkflowStep {
            name: "branch_b".to_string(),
            fighter_name: "FB".to_string(),
            prompt_template: "BranchB: {{input}}".to_string(),
            timeout_secs: Some(30),
            on_error: OnError::FailWorkflow,
            depends_on: vec![],
            condition: None,
            else_step: None,
            loop_config: None,
        },
        DagWorkflowStep {
            name: "merge".to_string(),
            fighter_name: "FM".to_string(),
            prompt_template: "Merge: A={{branch_a}} B={{branch_b}}".to_string(),
            timeout_secs: Some(30),
            on_error: OnError::FailWorkflow,
            depends_on: vec!["branch_a".to_string(), "branch_b".to_string()],
            condition: None,
            else_step: None,
            loop_config: None,
        },
    ];

    let result = execute_dag("parallel-test", &steps, "test input", executor).await;
    assert_eq!(result.status, WorkflowRunStatus::Completed);
    assert_eq!(result.step_results.len(), 3);

    // Merge step should reference both branches.
    let merge = &result.step_results["merge"];
    assert!(merge.response.contains("BranchA:"));
    assert!(merge.response.contains("BranchB:"));
}

/// Workflow with condition: step should be skipped when condition is false.
#[tokio::test]
async fn test_dag_condition_skip() {
    let executor: Arc<dyn StepExecutor> = Arc::new(EchoStepExecutor);

    let steps = vec![
        DagWorkflowStep {
            name: "producer".to_string(),
            fighter_name: "FP".to_string(),
            prompt_template: "Produced: {{input}}".to_string(),
            timeout_secs: Some(30),
            on_error: OnError::FailWorkflow,
            depends_on: vec![],
            condition: None,
            else_step: None,
            loop_config: None,
        },
        DagWorkflowStep {
            name: "conditional".to_string(),
            fighter_name: "FC".to_string(),
            prompt_template: "Conditional: {{input}}".to_string(),
            timeout_secs: Some(30),
            on_error: OnError::FailWorkflow,
            depends_on: vec!["producer".to_string()],
            // This condition will be false because the output won't contain "MAGIC"
            condition: Some(Condition::IfOutput {
                step: "producer".to_string(),
                contains: "MAGIC_KEYWORD_NOT_PRESENT".to_string(),
            }),
            else_step: None,
            loop_config: None,
        },
    ];

    let result = execute_dag("condition-test", &steps, "plain data", executor).await;
    assert_eq!(result.status, WorkflowRunStatus::Completed);

    // The conditional step should be skipped.
    let cond = &result.step_results["conditional"];
    assert_eq!(cond.status, StepStatus::Skipped);
}

/// Workflow with condition that evaluates to true.
#[tokio::test]
async fn test_dag_condition_runs() {
    let executor: Arc<dyn StepExecutor> = Arc::new(EchoStepExecutor);

    let steps = vec![
        DagWorkflowStep {
            name: "producer".to_string(),
            fighter_name: "FP".to_string(),
            prompt_template: "Produced: MAGIC {{input}}".to_string(),
            timeout_secs: Some(30),
            on_error: OnError::FailWorkflow,
            depends_on: vec![],
            condition: None,
            else_step: None,
            loop_config: None,
        },
        DagWorkflowStep {
            name: "conditional".to_string(),
            fighter_name: "FC".to_string(),
            prompt_template: "Conditional: {{input}}".to_string(),
            timeout_secs: Some(30),
            on_error: OnError::FailWorkflow,
            depends_on: vec!["producer".to_string()],
            condition: Some(Condition::IfOutput {
                step: "producer".to_string(),
                contains: "MAGIC".to_string(),
            }),
            else_step: None,
            loop_config: None,
        },
    ];

    let result = execute_dag("condition-runs-test", &steps, "data", executor).await;
    assert_eq!(result.status, WorkflowRunStatus::Completed);

    let cond = &result.step_results["conditional"];
    assert_eq!(cond.status, StepStatus::Completed);
}

/// DAG workflow failure with FailWorkflow error handler.
#[tokio::test]
async fn test_dag_step_failure() {
    let executor: Arc<dyn StepExecutor> = Arc::new(FailingStepExecutor {
        fail_step: "broken".to_string(),
    });

    let steps = vec![DagWorkflowStep {
        name: "broken".to_string(),
        fighter_name: "FB".to_string(),
        prompt_template: "{{input}}".to_string(),
        timeout_secs: Some(10),
        on_error: OnError::FailWorkflow,
        depends_on: vec![],
        condition: None,
        else_step: None,
        loop_config: None,
    }];

    let result = execute_dag("fail-dag", &steps, "data", executor).await;
    assert_eq!(result.status, WorkflowRunStatus::Failed);
}

// ---------------------------------------------------------------------------
// Tests: Variable substitution
// ---------------------------------------------------------------------------

/// Variable substitution with {{input}} and step references.
#[test]
fn test_expand_dag_variables_basic() {
    let mut results = HashMap::new();
    results.insert(
        "step_a".to_string(),
        StepResult {
            step_name: "step_a".to_string(),
            response: "alpha output".to_string(),
            tokens_used: 10,
            duration_ms: 5,
            error: None,
            status: StepStatus::Completed,
            started_at: None,
            completed_at: None,
        },
    );

    let expanded = expand_dag_variables(
        "Input: {{input}}, StepA: {{step_a}}, Explicit: {{step_a.output}}",
        "test input",
        "current",
        &results,
        None,
    );

    assert!(expanded.contains("Input: test input"));
    assert!(expanded.contains("StepA: alpha output"));
    assert!(expanded.contains("Explicit: alpha output"));
}

// ---------------------------------------------------------------------------
// Tests: Condition evaluation
// ---------------------------------------------------------------------------

/// evaluate_condition returns true for Always.
#[test]
fn test_condition_always_true() {
    let results = HashMap::new();
    assert!(evaluate_condition(&Condition::Always, &results));
}

/// evaluate_condition with IfSuccess.
#[test]
fn test_condition_if_success() {
    let mut results = HashMap::new();
    results.insert(
        "step1".to_string(),
        StepResult {
            step_name: "step1".to_string(),
            response: "ok".to_string(),
            tokens_used: 0,
            duration_ms: 0,
            error: None,
            status: StepStatus::Completed,
            started_at: None,
            completed_at: None,
        },
    );

    assert!(evaluate_condition(
        &Condition::IfSuccess {
            step: "step1".to_string()
        },
        &results
    ));

    // Non-existent step should be false.
    assert!(!evaluate_condition(
        &Condition::IfSuccess {
            step: "nonexistent".to_string()
        },
        &results
    ));
}

/// evaluate_condition with IfFailure.
#[test]
fn test_condition_if_failure() {
    let mut results = HashMap::new();
    results.insert(
        "failed_step".to_string(),
        StepResult {
            step_name: "failed_step".to_string(),
            response: "".to_string(),
            tokens_used: 0,
            duration_ms: 0,
            error: Some("boom".to_string()),
            status: StepStatus::Failed,
            started_at: None,
            completed_at: None,
        },
    );

    assert!(evaluate_condition(
        &Condition::IfFailure {
            step: "failed_step".to_string()
        },
        &results
    ));
}

// ---------------------------------------------------------------------------
// Tests: Circuit breaker
// ---------------------------------------------------------------------------

/// Circuit breaker opens after N failures.
#[test]
fn test_circuit_breaker_opens_after_failures() {
    let mut cb = CircuitBreakerState::default();
    let max = 3;
    let cooldown = 60;

    for _ in 0..max {
        cb.record_failure();
    }

    assert!(
        cb.is_open(max, cooldown),
        "circuit should be open after {} failures",
        max
    );
}

/// Circuit breaker resets on success.
#[test]
fn test_circuit_breaker_resets_on_success() {
    let mut cb = CircuitBreakerState::default();
    cb.record_failure();
    cb.record_failure();
    cb.record_success();

    assert!(
        !cb.is_open(3, 60),
        "circuit should be closed after success reset"
    );
    assert_eq!(cb.consecutive_failures, 0);
}
