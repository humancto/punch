//! Agent-to-Agent (A2A) HTTP wire protocol endpoints.
//!
//! Implements the A2A protocol over HTTP so agents can discover each other,
//! delegate tasks, and monitor task lifecycle across the network.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::Utc;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use punch_types::A2AClient;
use punch_types::a2a::{A2ARegistry, A2ATask, A2ATaskStatus, AgentCard, HttpA2AClient};

use crate::AppState;

// ---------------------------------------------------------------------------
// A2A State
// ---------------------------------------------------------------------------

/// Shared state for A2A protocol endpoints.
#[derive(Clone)]
pub struct A2AState {
    /// Registry of known remote agents.
    pub registry: Arc<A2ARegistry>,
    /// In-flight tasks indexed by task ID.
    pub tasks: Arc<DashMap<String, A2ATask>>,
    /// This agent's published card.
    pub local_card: AgentCard,
}

impl A2AState {
    /// Create a new A2A state with the given local agent card.
    pub fn new(local_card: AgentCard) -> Self {
        Self {
            registry: Arc::new(A2ARegistry::new()),
            tasks: Arc::new(DashMap::new()),
            local_card,
        }
    }
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

/// Request to send a new task to this agent.
#[derive(Debug, Deserialize)]
pub struct SendTaskRequest {
    /// Input payload for the task.
    pub input: serde_json::Value,
}

/// Request to register a remote agent by its base URL.
#[derive(Debug, Deserialize)]
pub struct RegisterAgentRequest {
    /// Base URL of the remote agent (e.g. "http://remote:3000").
    pub url: String,
}

/// Generic JSON error response.
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

/// Response for successful task cancellation.
#[derive(Debug, Serialize)]
struct CancelResponse {
    task_id: String,
    status: A2ATaskStatus,
}

/// Response for agent removal.
#[derive(Debug, Serialize)]
struct RemoveAgentResponse {
    removed: bool,
    agent_id: String,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the A2A protocol router.
///
/// These routes are mounted alongside the main API routes and implement the
/// full A2A wire protocol for agent discovery and task delegation.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/agent.json", get(get_agent_card))
        .route("/a2a/tasks/send", post(send_task))
        .route("/a2a/tasks/{task_id}", get(get_task))
        .route("/a2a/tasks/{task_id}/cancel", post(cancel_task))
        .route("/a2a/register", post(register_agent))
        .route("/a2a/agents", get(list_agents))
        .route("/a2a/agents/{agent_id}", delete(remove_agent))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /.well-known/agent.json -- serve this agent's public card.
///
/// This is the standard discovery endpoint. Remote agents fetch this to learn
/// about our capabilities, input/output modes, and how to send us tasks.
#[instrument(skip_all)]
async fn get_agent_card(State(state): State<AppState>) -> Json<AgentCard> {
    let a2a = a2a_state(&state);
    Json(a2a.local_card.clone())
}

/// POST /a2a/tasks/send -- receive a new task from a remote agent.
///
/// Creates a new task in Pending status and returns it with its assigned ID.
/// The task will be picked up for execution asynchronously.
#[instrument(skip_all)]
async fn send_task(
    State(state): State<AppState>,
    Json(body): Json<SendTaskRequest>,
) -> Result<(StatusCode, Json<A2ATask>), (StatusCode, Json<ErrorResponse>)> {
    let a2a = a2a_state(&state);
    let now = Utc::now();
    let task = A2ATask {
        id: Uuid::new_v4().to_string(),
        status: A2ATaskStatus::Pending,
        input: body.input,
        output: None,
        created_at: now,
        updated_at: now,
    };

    info!(task_id = %task.id, "received new A2A task");
    a2a.tasks.insert(task.id.clone(), task.clone());

    Ok((StatusCode::CREATED, Json(task)))
}

/// GET /a2a/tasks/:task_id -- get the current status and details of a task.
#[instrument(skip_all, fields(task_id = %task_id))]
async fn get_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<A2ATask>, (StatusCode, Json<ErrorResponse>)> {
    let a2a = a2a_state(&state);

    match a2a.tasks.get(&task_id) {
        Some(entry) => Ok(Json(entry.value().clone())),
        None => {
            warn!(task_id = %task_id, "task not found");
            Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("task {task_id} not found"),
                }),
            ))
        }
    }
}

/// POST /a2a/tasks/:task_id/cancel -- cancel a running or pending task.
///
/// Only tasks in Pending or Running status can be cancelled.
#[instrument(skip_all, fields(task_id = %task_id))]
async fn cancel_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<CancelResponse>, (StatusCode, Json<ErrorResponse>)> {
    let a2a = a2a_state(&state);

    match a2a.tasks.get_mut(&task_id) {
        Some(mut entry) => {
            let task = entry.value_mut();
            match task.status {
                A2ATaskStatus::Pending | A2ATaskStatus::Running => {
                    task.status = A2ATaskStatus::Cancelled;
                    task.updated_at = Utc::now();
                    info!(task_id = %task_id, "task cancelled");
                    Ok(Json(CancelResponse {
                        task_id,
                        status: A2ATaskStatus::Cancelled,
                    }))
                }
                _ => {
                    warn!(task_id = %task_id, status = ?task.status, "cannot cancel task in current status");
                    Err((
                        StatusCode::CONFLICT,
                        Json(ErrorResponse {
                            error: format!(
                                "task {task_id} cannot be cancelled (status: {:?})",
                                task.status
                            ),
                        }),
                    ))
                }
            }
        }
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("task {task_id} not found"),
            }),
        )),
    }
}

/// POST /a2a/register -- register a remote agent by fetching its agent card.
///
/// Takes a base URL, fetches the agent's card from its well-known endpoint,
/// and adds it to our registry.
#[instrument(skip_all)]
async fn register_agent(
    State(state): State<AppState>,
    Json(body): Json<RegisterAgentRequest>,
) -> Result<(StatusCode, Json<AgentCard>), (StatusCode, Json<ErrorResponse>)> {
    let a2a = a2a_state(&state);

    let client = HttpA2AClient::new().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("failed to create HTTP client: {e}"),
            }),
        )
    })?;

    let card = client.discover(&body.url).await.map_err(|e| {
        warn!(url = %body.url, error = %e, "failed to fetch remote agent card");
        (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!("failed to fetch agent card from {}: {e}", body.url),
            }),
        )
    })?;

    info!(agent = %card.name, url = %card.url, "registered remote agent");
    a2a.registry.register(card.clone());

    Ok((StatusCode::CREATED, Json(card)))
}

/// GET /a2a/agents -- list all known remote agents.
#[instrument(skip_all)]
async fn list_agents(State(state): State<AppState>) -> Json<Vec<AgentCard>> {
    let a2a = a2a_state(&state);
    Json(a2a.registry.list())
}

/// DELETE /a2a/agents/:agent_id -- unregister a remote agent by name.
#[instrument(skip_all, fields(agent_id = %agent_id))]
async fn remove_agent(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<Json<RemoveAgentResponse>, (StatusCode, Json<ErrorResponse>)> {
    let a2a = a2a_state(&state);

    if a2a.registry.remove(&agent_id) {
        info!(agent_id = %agent_id, "removed remote agent");
        Ok(Json(RemoveAgentResponse {
            removed: true,
            agent_id,
        }))
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("agent {agent_id} not found"),
            }),
        ))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract A2A state from the application state.
///
/// Uses the A2A state stored in the AppState. This is initialized when the
/// server starts.
fn a2a_state(state: &AppState) -> &A2AState {
    &state.a2a
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_card() -> AgentCard {
        AgentCard {
            name: "test-agent".to_string(),
            description: "A test agent".to_string(),
            url: "http://localhost:9999".to_string(),
            version: "0.1.0".to_string(),
            capabilities: vec!["test".to_string()],
            input_modes: vec!["text".to_string()],
            output_modes: vec!["text".to_string()],
            authentication: None,
        }
    }

    fn test_a2a_state() -> A2AState {
        A2AState::new(test_card())
    }

    #[test]
    fn test_a2a_state_creation() {
        let state = test_a2a_state();
        assert_eq!(state.local_card.name, "test-agent");
        assert_eq!(state.tasks.len(), 0);
        assert_eq!(state.registry.list().len(), 0);
    }

    #[test]
    fn test_task_insertion_and_retrieval() {
        let state = test_a2a_state();
        let now = Utc::now();
        let task = A2ATask {
            id: "task-001".to_string(),
            status: A2ATaskStatus::Pending,
            input: serde_json::json!({"prompt": "hello"}),
            output: None,
            created_at: now,
            updated_at: now,
        };

        state.tasks.insert(task.id.clone(), task);
        assert_eq!(state.tasks.len(), 1);

        let found = state.tasks.get("task-001").unwrap();
        assert_eq!(found.status, A2ATaskStatus::Pending);
    }

    #[test]
    fn test_task_cancel_lifecycle() {
        let state = test_a2a_state();
        let now = Utc::now();
        let task = A2ATask {
            id: "task-cancel".to_string(),
            status: A2ATaskStatus::Running,
            input: serde_json::json!({"prompt": "work"}),
            output: None,
            created_at: now,
            updated_at: now,
        };

        state.tasks.insert(task.id.clone(), task);

        // Cancel the task
        {
            let mut entry = state.tasks.get_mut("task-cancel").unwrap();
            entry.status = A2ATaskStatus::Cancelled;
            entry.updated_at = Utc::now();
        }

        let found = state.tasks.get("task-cancel").unwrap();
        assert_eq!(found.status, A2ATaskStatus::Cancelled);
    }

    #[test]
    fn test_task_completed_cannot_cancel() {
        let state = test_a2a_state();
        let now = Utc::now();
        let task = A2ATask {
            id: "task-done".to_string(),
            status: A2ATaskStatus::Completed,
            input: serde_json::json!({}),
            output: Some(serde_json::json!({"result": "ok"})),
            created_at: now,
            updated_at: now,
        };

        state.tasks.insert(task.id.clone(), task);

        let entry = state.tasks.get("task-done").unwrap();
        // Completed tasks should not be cancellable
        assert!(!matches!(
            entry.status,
            A2ATaskStatus::Pending | A2ATaskStatus::Running
        ));
    }

    #[test]
    fn test_registry_via_a2a_state() {
        let state = test_a2a_state();

        let card = AgentCard {
            name: "remote-fighter".to_string(),
            description: "A remote agent".to_string(),
            url: "http://remote:4000".to_string(),
            version: "1.0.0".to_string(),
            capabilities: vec!["code_review".to_string()],
            input_modes: vec!["text".to_string()],
            output_modes: vec!["text".to_string()],
            authentication: None,
        };

        state.registry.register(card);
        assert_eq!(state.registry.list().len(), 1);

        let found = state.registry.discover("remote-fighter");
        assert!(found.is_some());
        assert_eq!(found.unwrap().url, "http://remote:4000");
    }

    #[test]
    fn test_registry_remove_via_a2a_state() {
        let state = test_a2a_state();
        let card = test_card();
        state.registry.register(card);

        assert!(state.registry.remove("test-agent"));
        assert_eq!(state.registry.list().len(), 0);
        assert!(!state.registry.remove("test-agent"));
    }

    #[test]
    fn test_send_task_request_deserialization() {
        let json = r#"{"input": {"prompt": "analyze this code", "language": "rust"}}"#;
        let req: SendTaskRequest = serde_json::from_str(json).expect("deserialize");
        assert_eq!(req.input["prompt"], "analyze this code");
        assert_eq!(req.input["language"], "rust");
    }

    #[test]
    fn test_register_agent_request_deserialization() {
        let json = r#"{"url": "http://agent.example.com:3000"}"#;
        let req: RegisterAgentRequest = serde_json::from_str(json).expect("deserialize");
        assert_eq!(req.url, "http://agent.example.com:3000");
    }

    #[test]
    fn test_error_response_serialization() {
        let err = ErrorResponse {
            error: "task not found".to_string(),
        };
        let json = serde_json::to_string(&err).expect("serialize");
        assert!(json.contains("task not found"));
    }

    #[test]
    fn test_cancel_response_serialization() {
        let resp = CancelResponse {
            task_id: "t-123".to_string(),
            status: A2ATaskStatus::Cancelled,
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        assert!(json.contains("t-123"));
        assert!(json.contains("Cancelled"));
    }

    #[test]
    fn test_full_task_lifecycle() {
        let state = test_a2a_state();
        let now = Utc::now();

        // 1. Create task (simulating send_task handler)
        let task = A2ATask {
            id: "lifecycle-1".to_string(),
            status: A2ATaskStatus::Pending,
            input: serde_json::json!({"prompt": "test lifecycle"}),
            output: None,
            created_at: now,
            updated_at: now,
        };
        state.tasks.insert(task.id.clone(), task);

        // 2. Check status is Pending
        {
            let t = state.tasks.get("lifecycle-1").unwrap();
            assert_eq!(t.status, A2ATaskStatus::Pending);
        }

        // 3. Move to Running
        {
            let mut t = state.tasks.get_mut("lifecycle-1").unwrap();
            t.status = A2ATaskStatus::Running;
            t.updated_at = Utc::now();
        }
        {
            let t = state.tasks.get("lifecycle-1").unwrap();
            assert_eq!(t.status, A2ATaskStatus::Running);
        }

        // 4. Cancel
        {
            let mut t = state.tasks.get_mut("lifecycle-1").unwrap();
            t.status = A2ATaskStatus::Cancelled;
            t.updated_at = Utc::now();
        }
        {
            let t = state.tasks.get("lifecycle-1").unwrap();
            assert_eq!(t.status, A2ATaskStatus::Cancelled);
        }
    }

    #[test]
    fn test_multiple_tasks() {
        let state = test_a2a_state();
        let now = Utc::now();

        for i in 0..5 {
            let task = A2ATask {
                id: format!("multi-{i}"),
                status: A2ATaskStatus::Pending,
                input: serde_json::json!({"index": i}),
                output: None,
                created_at: now,
                updated_at: now,
            };
            state.tasks.insert(task.id.clone(), task);
        }

        assert_eq!(state.tasks.len(), 5);

        // Verify each task exists
        for i in 0..5 {
            let id = format!("multi-{i}");
            assert!(state.tasks.get(&id).is_some());
        }
    }

    #[test]
    fn test_unknown_task_not_found() {
        let state = test_a2a_state();
        assert!(state.tasks.get("nonexistent-task-id").is_none());
    }

    #[test]
    fn test_task_with_output() {
        let state = test_a2a_state();
        let now = Utc::now();
        let task = A2ATask {
            id: "task-with-output".to_string(),
            status: A2ATaskStatus::Completed,
            input: serde_json::json!({"prompt": "generate"}),
            output: Some(serde_json::json!({"result": "generated text", "tokens": 42})),
            created_at: now,
            updated_at: now,
        };
        state.tasks.insert(task.id.clone(), task);

        let found = state.tasks.get("task-with-output").unwrap();
        assert_eq!(found.status, A2ATaskStatus::Completed);
        assert!(found.output.is_some());
        assert_eq!(found.output.as_ref().unwrap()["tokens"], 42);
    }

    #[test]
    fn test_cancel_pending_task() {
        let state = test_a2a_state();
        let now = Utc::now();
        let task = A2ATask {
            id: "pending-cancel".to_string(),
            status: A2ATaskStatus::Pending,
            input: serde_json::json!({}),
            output: None,
            created_at: now,
            updated_at: now,
        };
        state.tasks.insert(task.id.clone(), task);

        // Pending tasks should be cancellable
        {
            let entry = state.tasks.get("pending-cancel").unwrap();
            assert!(matches!(
                entry.status,
                A2ATaskStatus::Pending | A2ATaskStatus::Running
            ));
        }

        // Cancel it
        {
            let mut entry = state.tasks.get_mut("pending-cancel").unwrap();
            entry.status = A2ATaskStatus::Cancelled;
        }

        let found = state.tasks.get("pending-cancel").unwrap();
        assert_eq!(found.status, A2ATaskStatus::Cancelled);
    }

    #[test]
    fn test_remove_agent_response_serialization() {
        let resp = RemoveAgentResponse {
            removed: true,
            agent_id: "remote-agent-1".to_string(),
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        assert!(json.contains("true"));
        assert!(json.contains("remote-agent-1"));
    }

    #[test]
    fn test_remove_agent_response_not_found() {
        let resp = RemoveAgentResponse {
            removed: false,
            agent_id: "missing-agent".to_string(),
        };
        let json = serde_json::to_string(&resp).expect("serialize");
        assert!(json.contains("false"));
    }

    #[test]
    fn test_registry_discover_unknown_agent() {
        let state = test_a2a_state();
        assert!(state.registry.discover("nonexistent-agent").is_none());
    }

    #[test]
    fn test_registry_multiple_agents() {
        let state = test_a2a_state();

        for i in 0..3 {
            let card = AgentCard {
                name: format!("agent-{i}"),
                description: format!("Agent {i}"),
                url: format!("http://agent-{i}:4000"),
                version: "1.0.0".to_string(),
                capabilities: vec![],
                input_modes: vec!["text".to_string()],
                output_modes: vec!["text".to_string()],
                authentication: None,
            };
            state.registry.register(card);
        }

        assert_eq!(state.registry.list().len(), 3);

        for i in 0..3 {
            let name = format!("agent-{i}");
            assert!(state.registry.discover(&name).is_some());
        }
    }

    #[test]
    fn test_a2a_state_clone() {
        let state = test_a2a_state();
        let now = Utc::now();
        let task = A2ATask {
            id: "clone-task".to_string(),
            status: A2ATaskStatus::Pending,
            input: serde_json::json!({}),
            output: None,
            created_at: now,
            updated_at: now,
        };
        state.tasks.insert(task.id.clone(), task);

        // Clone shares the same underlying Arc
        let cloned = state.clone();
        assert_eq!(cloned.tasks.len(), 1);
        assert_eq!(cloned.local_card.name, "test-agent");
    }

    #[test]
    fn test_task_updated_at_changes() {
        let state = test_a2a_state();
        let now = Utc::now();
        let task = A2ATask {
            id: "timestamp-test".to_string(),
            status: A2ATaskStatus::Pending,
            input: serde_json::json!({}),
            output: None,
            created_at: now,
            updated_at: now,
        };
        state.tasks.insert(task.id.clone(), task);

        let original_updated = {
            let t = state.tasks.get("timestamp-test").unwrap();
            t.updated_at
        };

        // Simulate status change
        std::thread::sleep(std::time::Duration::from_millis(10));
        {
            let mut t = state.tasks.get_mut("timestamp-test").unwrap();
            t.status = A2ATaskStatus::Running;
            t.updated_at = Utc::now();
        }

        let new_updated = {
            let t = state.tasks.get("timestamp-test").unwrap();
            t.updated_at
        };

        assert!(new_updated > original_updated);
    }

    #[test]
    fn test_task_removal() {
        let state = test_a2a_state();
        let now = Utc::now();
        let task = A2ATask {
            id: "removable".to_string(),
            status: A2ATaskStatus::Pending,
            input: serde_json::json!({}),
            output: None,
            created_at: now,
            updated_at: now,
        };
        state.tasks.insert(task.id.clone(), task);
        assert_eq!(state.tasks.len(), 1);

        state.tasks.remove("removable");
        assert_eq!(state.tasks.len(), 0);
    }

    #[test]
    fn test_send_task_request_complex_input() {
        let json = r#"{"input": {"messages": [{"role": "user", "content": "hello"}], "config": {"temperature": 0.7}}}"#;
        let req: SendTaskRequest = serde_json::from_str(json).expect("deserialize");
        assert!(req.input["messages"].is_array());
        assert_eq!(req.input["config"]["temperature"], 0.7);
    }

    #[test]
    fn test_agent_card_capabilities() {
        let card = AgentCard {
            name: "multi-cap-agent".to_string(),
            description: "Agent with multiple capabilities".to_string(),
            url: "http://localhost:5000".to_string(),
            version: "2.0.0".to_string(),
            capabilities: vec![
                "code_review".to_string(),
                "testing".to_string(),
                "deployment".to_string(),
            ],
            input_modes: vec!["text".to_string(), "json".to_string()],
            output_modes: vec!["text".to_string(), "json".to_string()],
            authentication: None,
        };

        assert_eq!(card.capabilities.len(), 3);
        assert!(card.capabilities.contains(&"code_review".to_string()));
        assert_eq!(card.input_modes.len(), 2);
        assert_eq!(card.output_modes.len(), 2);
    }

    #[test]
    fn test_failed_task_status() {
        let state = test_a2a_state();
        let now = Utc::now();
        let task = A2ATask {
            id: "failed-task".to_string(),
            status: A2ATaskStatus::Failed("model timeout".to_string()),
            input: serde_json::json!({"prompt": "something"}),
            output: Some(serde_json::json!({"error": "model timeout"})),
            created_at: now,
            updated_at: now,
        };
        state.tasks.insert(task.id.clone(), task);

        let found = state.tasks.get("failed-task").unwrap();
        assert!(matches!(found.status, A2ATaskStatus::Failed(_)));
        // Failed tasks should not be cancellable
        assert!(!matches!(
            found.status,
            A2ATaskStatus::Pending | A2ATaskStatus::Running
        ));
    }
}
