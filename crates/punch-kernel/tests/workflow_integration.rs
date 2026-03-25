//! Integration test: multi-step workflow execution end-to-end.
//!
//! Uses a mock LLM driver that returns canned responses so we can validate
//! the full workflow lifecycle without making real API calls.
//!
//! Tests cover:
//! - Multi-step pipeline execution with output flowing between steps
//! - Variable substitution ({{input}}, {{previous_output}}, {{step_N}}, {{step_name}})
//! - OnError::SkipStep behaviour
//! - OnError::RetryOnce behaviour
//! - Final run status tracking

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;

use punch_kernel::{OnError, Ring, Workflow, WorkflowId, WorkflowRunStatus, WorkflowStep};
use punch_memory::MemorySubstrate;
use punch_runtime::{CompletionRequest, CompletionResponse, LlmDriver, StopReason, TokenUsage};
use punch_types::{ModelConfig, Provider, PunchConfig, PunchResult};

// ---------------------------------------------------------------------------
// Mock LLM Driver
// ---------------------------------------------------------------------------

/// A mock LLM driver that echoes back the user message with a prefix,
/// so we can verify variable substitution and output chaining.
struct MockLlmDriver {
    call_count: AtomicU64,
}

impl MockLlmDriver {
    fn new() -> Self {
        Self {
            call_count: AtomicU64::new(0),
        }
    }

    fn calls(&self) -> u64 {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmDriver for MockLlmDriver {
    async fn complete(&self, request: CompletionRequest) -> PunchResult<CompletionResponse> {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);

        // Extract the user message (last user message in history).
        let user_content = request
            .messages
            .iter()
            .rev()
            .find(|m| m.role == punch_types::Role::User)
            .map(|m| m.content.clone())
            .unwrap_or_default();

        // Echo back with a step-specific prefix so tests can verify chaining.
        let response = format!("[mock-response-{}] {}", count, user_content);

        Ok(CompletionResponse {
            message: punch_types::Message {
                role: punch_types::Role::Assistant,
                content: response,
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

/// A mock driver that always fails (for error handling tests).
struct FailingLlmDriver;

#[async_trait]
impl LlmDriver for FailingLlmDriver {
    async fn complete(&self, _request: CompletionRequest) -> PunchResult<CompletionResponse> {
        Err(punch_types::PunchError::Provider {
            provider: "mock".to_string(),
            message: "intentional test failure".to_string(),
        })
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
            model: "gpt-oss:20b".to_string(),
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
// Tests
// ---------------------------------------------------------------------------

/// Test: 2-step pipeline where step 1 output feeds into step 2 input.
#[tokio::test]
async fn test_workflow_two_step_pipeline() {
    let driver = Arc::new(MockLlmDriver::new());
    let ring = create_ring(Arc::clone(&driver) as Arc<dyn LlmDriver>);

    let workflow = Workflow {
        id: WorkflowId::new(),
        name: "two-step-test".to_string(),
        steps: vec![
            WorkflowStep {
                name: "analyze".to_string(),
                fighter_name: "Analyzer".to_string(),
                prompt_template: "Analyze: {{input}}".to_string(),
                timeout_secs: Some(30),
                on_error: OnError::FailWorkflow,
            },
            WorkflowStep {
                name: "summarize".to_string(),
                fighter_name: "Summarizer".to_string(),
                prompt_template: "Summarize: {{input}}".to_string(),
                timeout_secs: Some(30),
                on_error: OnError::FailWorkflow,
            },
        ],
    };

    let wf_id = ring.register_workflow(workflow);
    let run_id = ring
        .execute_workflow(&wf_id, "Rust programming language".to_string())
        .await
        .expect("workflow should execute");

    // Verify the run completed.
    let run = ring
        .workflow_engine()
        .get_run(&run_id)
        .expect("run should exist");

    assert_eq!(run.status, WorkflowRunStatus::Completed);
    assert_eq!(run.step_results.len(), 2);
    assert_eq!(run.step_results[0].step_name, "analyze");
    assert_eq!(run.step_results[1].step_name, "summarize");

    // Step 1 should have received the original input.
    assert!(
        run.step_results[0]
            .response
            .contains("Analyze: Rust programming language"),
        "Step 1 should contain the original input. Got: {}",
        run.step_results[0].response
    );

    // Step 2's input ({{input}}) is the output of step 1.
    // So it should contain "Summarize: [mock-response-0] Analyze: ..."
    assert!(
        run.step_results[1].response.contains("Summarize:"),
        "Step 2 should contain 'Summarize:'. Got: {}",
        run.step_results[1].response
    );
    assert!(
        run.step_results[1].response.contains("mock-response-0"),
        "Step 2 should reference step 1's output. Got: {}",
        run.step_results[1].response
    );

    // Both steps should report tokens.
    assert!(run.step_results[0].tokens_used > 0);
    assert!(run.step_results[1].tokens_used > 0);

    // Driver should have been called twice (once per step).
    assert_eq!(driver.calls(), 2);

    ring.shutdown();
}

/// Test: variable substitution with {{previous_output}} alias.
#[tokio::test]
async fn test_workflow_previous_output_variable() {
    let driver = Arc::new(MockLlmDriver::new());
    let ring = create_ring(Arc::clone(&driver) as Arc<dyn LlmDriver>);

    let workflow = Workflow {
        id: WorkflowId::new(),
        name: "prev-output-test".to_string(),
        steps: vec![
            WorkflowStep {
                name: "step1".to_string(),
                fighter_name: "Fighter1".to_string(),
                prompt_template: "Process: {{input}}".to_string(),
                timeout_secs: Some(30),
                on_error: OnError::FailWorkflow,
            },
            WorkflowStep {
                name: "step2".to_string(),
                fighter_name: "Fighter2".to_string(),
                prompt_template: "Continue from: {{previous_output}}".to_string(),
                timeout_secs: Some(30),
                on_error: OnError::FailWorkflow,
            },
        ],
    };

    let wf_id = ring.register_workflow(workflow);
    let run_id = ring
        .execute_workflow(&wf_id, "test data".to_string())
        .await
        .expect("workflow should execute");

    let run = ring.workflow_engine().get_run(&run_id).unwrap();
    assert_eq!(run.status, WorkflowRunStatus::Completed);

    // Step 2 should contain the resolved {{previous_output}} (= step 1's response)
    assert!(
        run.step_results[1].response.contains("Continue from:"),
        "Step 2 should use previous_output. Got: {}",
        run.step_results[1].response
    );
    assert!(
        run.step_results[1].response.contains("mock-response-0"),
        "Step 2 should contain step 1's output. Got: {}",
        run.step_results[1].response
    );

    ring.shutdown();
}

/// Test: variable substitution with {{step_1}} numeric reference.
#[tokio::test]
async fn test_workflow_step_number_variable() {
    let driver = Arc::new(MockLlmDriver::new());
    let ring = create_ring(Arc::clone(&driver) as Arc<dyn LlmDriver>);

    let workflow = Workflow {
        id: WorkflowId::new(),
        name: "step-ref-test".to_string(),
        steps: vec![
            WorkflowStep {
                name: "research".to_string(),
                fighter_name: "Researcher".to_string(),
                prompt_template: "Research: {{input}}".to_string(),
                timeout_secs: Some(30),
                on_error: OnError::FailWorkflow,
            },
            WorkflowStep {
                name: "combine".to_string(),
                fighter_name: "Combiner".to_string(),
                prompt_template: "Combine step1={{step_1}} with input={{input}}".to_string(),
                timeout_secs: Some(30),
                on_error: OnError::FailWorkflow,
            },
        ],
    };

    let wf_id = ring.register_workflow(workflow);
    let run_id = ring
        .execute_workflow(&wf_id, "quantum computing".to_string())
        .await
        .expect("workflow should execute");

    let run = ring.workflow_engine().get_run(&run_id).unwrap();
    assert_eq!(run.status, WorkflowRunStatus::Completed);

    // Step 2 should have {{step_1}} resolved to step 1's response
    assert!(
        run.step_results[1].response.contains("step1="),
        "Step 2 should contain 'step1='. Got: {}",
        run.step_results[1].response
    );

    ring.shutdown();
}

/// Test: OnError::SkipStep skips a failing step and continues.
#[tokio::test]
async fn test_workflow_skip_step_on_error() {
    let driver: Arc<dyn LlmDriver> = Arc::new(FailingLlmDriver);
    let ring = create_ring(driver);

    let workflow = Workflow {
        id: WorkflowId::new(),
        name: "skip-test".to_string(),
        steps: vec![
            WorkflowStep {
                name: "failing_step".to_string(),
                fighter_name: "Failer".to_string(),
                prompt_template: "This will fail: {{input}}".to_string(),
                timeout_secs: Some(10),
                on_error: OnError::SkipStep,
            },
            WorkflowStep {
                name: "recovery_step".to_string(),
                fighter_name: "Recoverer".to_string(),
                prompt_template: "Recover: {{input}}".to_string(),
                timeout_secs: Some(10),
                on_error: OnError::FailWorkflow,
            },
        ],
    };

    let wf_id = ring.register_workflow(workflow);
    let run_id = ring
        .execute_workflow(&wf_id, "test input".to_string())
        .await
        .expect("workflow should execute (skip step)");

    let run = ring.workflow_engine().get_run(&run_id).unwrap();

    // Step 1 was skipped (error), step 2 also fails (FailingDriver), so workflow fails.
    // But step 1 should be marked as skipped with an error.
    assert_eq!(run.step_results.len(), 2);
    assert!(
        run.step_results[0].error.is_some(),
        "Step 1 should have an error"
    );
    assert_eq!(run.step_results[0].response, "");

    // The workflow should fail because step 2 also uses the failing driver
    // and has OnError::FailWorkflow.
    assert_eq!(run.status, WorkflowRunStatus::Failed);

    ring.shutdown();
}

/// Test: OnError::FailWorkflow stops the entire pipeline.
#[tokio::test]
async fn test_workflow_fail_workflow_on_error() {
    let driver: Arc<dyn LlmDriver> = Arc::new(FailingLlmDriver);
    let ring = create_ring(driver);

    let workflow = Workflow {
        id: WorkflowId::new(),
        name: "fail-test".to_string(),
        steps: vec![
            WorkflowStep {
                name: "failing_step".to_string(),
                fighter_name: "Failer".to_string(),
                prompt_template: "{{input}}".to_string(),
                timeout_secs: Some(10),
                on_error: OnError::FailWorkflow,
            },
            WorkflowStep {
                name: "never_reached".to_string(),
                fighter_name: "Unreachable".to_string(),
                prompt_template: "{{input}}".to_string(),
                timeout_secs: Some(10),
                on_error: OnError::FailWorkflow,
            },
        ],
    };

    let wf_id = ring.register_workflow(workflow);
    let run_id = ring
        .execute_workflow(&wf_id, "test".to_string())
        .await
        .expect("workflow should return run_id even on failure");

    let run = ring.workflow_engine().get_run(&run_id).unwrap();
    assert_eq!(run.status, WorkflowRunStatus::Failed);
    // Only the first step should have a result (the second was never reached).
    assert_eq!(run.step_results.len(), 1);
    assert!(run.step_results[0].error.is_some());

    ring.shutdown();
}

/// Test: OnError::RetryOnce retries a step once before failing.
#[tokio::test]
async fn test_workflow_retry_once_on_error() {
    let driver: Arc<dyn LlmDriver> = Arc::new(FailingLlmDriver);
    let ring = create_ring(driver);

    let workflow = Workflow {
        id: WorkflowId::new(),
        name: "retry-test".to_string(),
        steps: vec![WorkflowStep {
            name: "retry_step".to_string(),
            fighter_name: "Retrier".to_string(),
            prompt_template: "{{input}}".to_string(),
            timeout_secs: Some(10),
            on_error: OnError::RetryOnce,
        }],
    };

    let wf_id = ring.register_workflow(workflow);
    let run_id = ring
        .execute_workflow(&wf_id, "test".to_string())
        .await
        .expect("workflow should return run_id");

    let run = ring.workflow_engine().get_run(&run_id).unwrap();
    // Should fail after retry.
    assert_eq!(run.status, WorkflowRunStatus::Failed);
    assert_eq!(run.step_results.len(), 1);
    assert!(run.step_results[0].error.is_some());

    ring.shutdown();
}

/// Test: workflow run status tracking and listing.
#[tokio::test]
async fn test_workflow_run_listing() {
    let driver = Arc::new(MockLlmDriver::new());
    let ring = create_ring(Arc::clone(&driver) as Arc<dyn LlmDriver>);

    let workflow = Workflow {
        id: WorkflowId::new(),
        name: "list-test".to_string(),
        steps: vec![WorkflowStep {
            name: "only_step".to_string(),
            fighter_name: "Fighter".to_string(),
            prompt_template: "{{input}}".to_string(),
            timeout_secs: Some(30),
            on_error: OnError::FailWorkflow,
        }],
    };

    let wf_id = ring.register_workflow(workflow);

    // Execute the workflow twice.
    let run1 = ring
        .execute_workflow(&wf_id, "first".to_string())
        .await
        .unwrap();
    let run2 = ring
        .execute_workflow(&wf_id, "second".to_string())
        .await
        .unwrap();

    // List all runs.
    let all_runs = ring.workflow_engine().list_runs();
    assert_eq!(all_runs.len(), 2);

    // List runs for this workflow.
    let wf_runs = ring.workflow_engine().list_runs_for_workflow(&wf_id);
    assert_eq!(wf_runs.len(), 2);

    // Get individual runs.
    let r1 = ring.workflow_engine().get_run(&run1).unwrap();
    assert_eq!(r1.status, WorkflowRunStatus::Completed);
    assert!(r1.completed_at.is_some());

    let r2 = ring.workflow_engine().get_run(&run2).unwrap();
    assert_eq!(r2.status, WorkflowRunStatus::Completed);

    // List workflows.
    let workflows = ring.workflow_engine().list_workflows();
    assert_eq!(workflows.len(), 1);
    assert_eq!(workflows[0].name, "list-test");

    ring.shutdown();
}

/// Test: OnError::SkipStep with a mock driver where only the first step fails.
/// Uses a custom driver that fails on the first call but succeeds on the second.
#[tokio::test]
async fn test_workflow_skip_step_with_recovery() {
    /// Driver that fails on even-numbered calls and succeeds on odd ones.
    struct AlternatingDriver {
        call_count: AtomicU64,
    }

    #[async_trait]
    impl LlmDriver for AlternatingDriver {
        async fn complete(&self, request: CompletionRequest) -> PunchResult<CompletionResponse> {
            let count = self.call_count.fetch_add(1, Ordering::SeqCst);

            if count.is_multiple_of(2) {
                // Fail on even calls (0, 2, 4...)
                Err(punch_types::PunchError::Provider {
                    provider: "mock".to_string(),
                    message: "alternating failure".to_string(),
                })
            } else {
                // Succeed on odd calls (1, 3, 5...)
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
                        content: format!("[recovered] {}", user_content),
                        tool_calls: Vec::new(),
                        tool_results: Vec::new(),
                        timestamp: chrono::Utc::now(),
                    },
                    usage: TokenUsage {
                        input_tokens: 30,
                        output_tokens: 15,
                    },
                    stop_reason: StopReason::EndTurn,
                })
            }
        }
    }

    let driver: Arc<dyn LlmDriver> = Arc::new(AlternatingDriver {
        call_count: AtomicU64::new(0),
    });
    let ring = create_ring(driver);

    let workflow = Workflow {
        id: WorkflowId::new(),
        name: "skip-recover-test".to_string(),
        steps: vec![
            WorkflowStep {
                name: "fragile_step".to_string(),
                fighter_name: "Fragile".to_string(),
                prompt_template: "Process: {{input}}".to_string(),
                timeout_secs: Some(30),
                on_error: OnError::SkipStep,
            },
            WorkflowStep {
                name: "stable_step".to_string(),
                fighter_name: "Stable".to_string(),
                prompt_template: "Finalize: {{input}}".to_string(),
                timeout_secs: Some(30),
                on_error: OnError::FailWorkflow,
            },
        ],
    };

    let wf_id = ring.register_workflow(workflow);
    let run_id = ring
        .execute_workflow(&wf_id, "important data".to_string())
        .await
        .expect("workflow should execute");

    let run = ring.workflow_engine().get_run(&run_id).unwrap();

    // Step 1 fails (call 0 = even = failure) and is skipped.
    // Step 2 succeeds (call 1 = odd = success).
    assert_eq!(run.status, WorkflowRunStatus::Completed);
    assert_eq!(run.step_results.len(), 2);

    // Step 1 was skipped with error.
    assert!(run.step_results[0].error.is_some());
    assert_eq!(run.step_results[0].response, "");

    // Step 2 succeeded. Its {{input}} should be the original input
    // (since step 1 was skipped and produced empty output, current_input stays).
    assert!(run.step_results[1].error.is_none());
    assert!(
        run.step_results[1].response.contains("recovered"),
        "Step 2 should have recovered. Got: {}",
        run.step_results[1].response
    );

    ring.shutdown();
}
