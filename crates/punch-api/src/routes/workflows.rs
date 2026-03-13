//! Workflow management endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use punch_kernel::{
    OnError, Workflow, WorkflowId, WorkflowRun, WorkflowRunId, WorkflowRunStatus, WorkflowStep,
};

use crate::AppState;

/// Build the workflow routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/workflows", post(create_workflow).get(list_workflows))
        .route("/api/workflows/{id}/execute", post(execute_workflow))
        .route("/api/workflows/{id}/runs", get(list_runs))
        .route("/api/workflows/{id}/runs/{run_id}", get(get_run))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateWorkflowRequest {
    name: String,
    steps: Vec<CreateWorkflowStep>,
}

#[derive(Deserialize)]
struct CreateWorkflowStep {
    name: String,
    fighter_name: String,
    prompt_template: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    on_error: Option<String>,
}

#[derive(Serialize)]
struct CreateWorkflowResponse {
    id: WorkflowId,
    name: String,
}

#[derive(Serialize)]
struct WorkflowSummary {
    id: WorkflowId,
    name: String,
    step_count: usize,
}

#[derive(Deserialize)]
struct ExecuteWorkflowRequest {
    input: String,
}

#[derive(Serialize)]
struct ExecuteWorkflowResponse {
    run_id: WorkflowRunId,
    status: WorkflowRunStatus,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/workflows — register a new workflow.
#[instrument(skip_all)]
async fn create_workflow(
    State(state): State<AppState>,
    Json(body): Json<CreateWorkflowRequest>,
) -> (StatusCode, Json<CreateWorkflowResponse>) {
    let steps: Vec<WorkflowStep> = body
        .steps
        .into_iter()
        .map(|s| {
            let on_error = match s.on_error.as_deref() {
                Some("skip_step") => OnError::SkipStep,
                Some("retry_once") => OnError::RetryOnce,
                _ => OnError::FailWorkflow,
            };
            WorkflowStep {
                name: s.name,
                fighter_name: s.fighter_name,
                prompt_template: s.prompt_template,
                timeout_secs: s.timeout_secs,
                on_error,
            }
        })
        .collect();

    let workflow = Workflow {
        id: WorkflowId::new(),
        name: body.name.clone(),
        steps,
    };

    let id = state.ring.register_workflow(workflow);

    (
        StatusCode::CREATED,
        Json(CreateWorkflowResponse {
            id,
            name: body.name,
        }),
    )
}

/// GET /api/workflows — list all registered workflows.
#[instrument(skip_all)]
async fn list_workflows(State(state): State<AppState>) -> Json<Vec<WorkflowSummary>> {
    let workflows = state.ring.workflow_engine().list_workflows();

    let summaries = workflows
        .into_iter()
        .map(|w| WorkflowSummary {
            id: w.id,
            name: w.name,
            step_count: w.steps.len(),
        })
        .collect();

    Json(summaries)
}

/// POST /api/workflows/:id/execute — execute a workflow with input.
#[instrument(skip(state, body))]
async fn execute_workflow(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ExecuteWorkflowRequest>,
) -> Result<Json<ExecuteWorkflowResponse>, (StatusCode, Json<ErrorResponse>)> {
    let workflow_id = WorkflowId(id);

    let run_id = state
        .ring
        .execute_workflow(&workflow_id, body.input)
        .await
        .map_err(|e| {
            let status = match &e {
                punch_types::PunchError::Internal(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (
                status,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    // Get the run to return the status.
    let status = state
        .ring
        .workflow_engine()
        .get_run(&run_id)
        .map(|r| r.status)
        .unwrap_or(WorkflowRunStatus::Completed);

    Ok(Json(ExecuteWorkflowResponse { run_id, status }))
}

/// GET /api/workflows/:id/runs — list runs for a workflow.
#[instrument(skip(state))]
async fn list_runs(State(state): State<AppState>, Path(id): Path<Uuid>) -> Json<Vec<WorkflowRun>> {
    let workflow_id = WorkflowId(id);
    let runs = state
        .ring
        .workflow_engine()
        .list_runs_for_workflow(&workflow_id);
    Json(runs)
}

/// GET /api/workflows/:id/runs/:run_id — get run status and results.
#[instrument(skip(state))]
async fn get_run(
    State(state): State<AppState>,
    Path((_, run_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<WorkflowRun>, (StatusCode, Json<ErrorResponse>)> {
    let wf_run_id = WorkflowRunId(run_id);

    state
        .ring
        .workflow_engine()
        .get_run(&wf_run_id)
        .map(Json)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("workflow run {} not found", run_id),
                }),
            )
        })
}
