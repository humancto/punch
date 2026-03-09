//! Trigger management endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use punch_kernel::triggers::{Trigger, TriggerAction, TriggerCondition, TriggerId};

use crate::AppState;

/// Build the trigger routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/triggers", post(register_trigger).get(list_triggers))
        .route("/api/triggers/{id}", axum::routing::delete(remove_trigger))
        .route(
            "/api/triggers/webhook/{id}",
            post(receive_webhook),
        )
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct RegisterTriggerRequest {
    name: String,
    condition: TriggerCondition,
    action: TriggerAction,
    #[serde(default)]
    max_fires: u64,
}

#[derive(Serialize)]
struct RegisterTriggerResponse {
    id: TriggerId,
    name: String,
}

#[derive(Serialize)]
struct TriggerListItem {
    id: TriggerId,
    name: String,
    condition_type: String,
    enabled: bool,
    fire_count: u64,
    created_at: chrono::DateTime<Utc>,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize)]
struct WebhookResponse {
    status: String,
    action: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/triggers — register a new trigger.
#[instrument(skip_all)]
async fn register_trigger(
    State(state): State<AppState>,
    Json(body): Json<RegisterTriggerRequest>,
) -> (StatusCode, Json<RegisterTriggerResponse>) {
    let trigger = Trigger {
        id: TriggerId::new(),
        name: body.name.clone(),
        condition: body.condition,
        action: body.action,
        enabled: true,
        created_at: Utc::now(),
        fire_count: 0,
        max_fires: body.max_fires,
    };

    let id = state.ring.register_trigger(trigger);

    (
        StatusCode::CREATED,
        Json(RegisterTriggerResponse {
            id,
            name: body.name,
        }),
    )
}

/// GET /api/triggers — list all triggers.
#[instrument(skip_all)]
async fn list_triggers(State(state): State<AppState>) -> Json<Vec<TriggerListItem>> {
    let triggers = state.ring.list_triggers();

    let items = triggers
        .into_iter()
        .map(|(id, summary)| TriggerListItem {
            id,
            name: summary.name,
            condition_type: summary.condition_type,
            enabled: summary.enabled,
            fire_count: summary.fire_count,
            created_at: summary.created_at,
        })
        .collect();

    Json(items)
}

/// DELETE /api/triggers/:id — remove a trigger.
#[instrument(skip(state))]
async fn remove_trigger(State(state): State<AppState>, Path(id): Path<Uuid>) -> StatusCode {
    let trigger_id = TriggerId(id);
    state.ring.remove_trigger(&trigger_id);
    StatusCode::NO_CONTENT
}

/// POST /api/triggers/webhook/:id — webhook receiver endpoint.
#[instrument(skip(state))]
async fn receive_webhook(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<WebhookResponse>, (StatusCode, Json<ErrorResponse>)> {
    let trigger_id = TriggerId(id);

    let action = state
        .ring
        .trigger_engine()
        .check_webhook(&trigger_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("webhook trigger {} not found or disabled", id),
                }),
            )
        })?;

    let action_desc = format!("{:?}", action);

    Ok(Json(WebhookResponse {
        status: "triggered".to_string(),
        action: action_desc,
    }))
}
