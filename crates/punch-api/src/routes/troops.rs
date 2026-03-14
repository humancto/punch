//! Troop management endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use punch_types::{
    CoordinationStrategy, FighterId, Troop, TroopId, TroopStatus,
};

use crate::AppState;

/// Build the troop routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/troops", post(form_troop).get(list_troops))
        .route("/api/troops/{id}", get(get_troop).delete(disband_troop))
        .route("/api/troops/{id}/tasks", post(assign_task))
        .route(
            "/api/troops/{id}/members",
            post(recruit_member),
        )
        .route(
            "/api/troops/{troop_id}/members/{fighter_id}",
            axum::routing::delete(dismiss_member),
        )
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct FormTroopRequest {
    name: String,
    leader: Uuid,
    members: Vec<Uuid>,
    strategy: CoordinationStrategy,
}

#[derive(Serialize)]
struct FormTroopResponse {
    id: TroopId,
    name: String,
}

#[derive(Serialize)]
struct TroopSummary {
    id: TroopId,
    name: String,
    leader: FighterId,
    member_count: usize,
    strategy: CoordinationStrategy,
    status: TroopStatus,
}

#[derive(Deserialize)]
struct AssignTaskRequest {
    task: String,
}

#[derive(Serialize)]
struct AssignTaskResponse {
    assigned_to: Vec<FighterId>,
}

#[derive(Deserialize)]
struct RecruitRequest {
    fighter_id: Uuid,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/troops -- form a new troop.
#[instrument(skip_all)]
async fn form_troop(
    State(state): State<AppState>,
    Json(body): Json<FormTroopRequest>,
) -> Result<(StatusCode, Json<FormTroopResponse>), (StatusCode, Json<ErrorResponse>)> {
    let leader = FighterId(body.leader);
    let members: Vec<FighterId> = body.members.into_iter().map(FighterId).collect();

    let troop_id = state
        .ring
        .form_troop(body.name.clone(), leader, members, body.strategy)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok((
        StatusCode::CREATED,
        Json(FormTroopResponse {
            id: troop_id,
            name: body.name,
        }),
    ))
}

/// GET /api/troops -- list all troops.
#[instrument(skip_all)]
async fn list_troops(State(state): State<AppState>) -> Json<Vec<TroopSummary>> {
    let troops = state.ring.list_troops();
    let summaries = troops
        .into_iter()
        .map(|t| TroopSummary {
            id: t.id,
            name: t.name,
            leader: t.leader,
            member_count: t.members.len(),
            strategy: t.strategy,
            status: t.status,
        })
        .collect();
    Json(summaries)
}

/// GET /api/troops/:id -- get troop details.
#[instrument(skip(state))]
async fn get_troop(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Troop>, (StatusCode, Json<ErrorResponse>)> {
    let troop_id = TroopId(id);
    let troop = state.ring.get_troop_status(&troop_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("troop {} not found", id),
            }),
        )
    })?;
    Ok(Json(troop))
}

/// POST /api/troops/:id/tasks -- assign a task to a troop.
#[instrument(skip(state, body))]
async fn assign_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<AssignTaskRequest>,
) -> Result<Json<AssignTaskResponse>, (StatusCode, Json<ErrorResponse>)> {
    let troop_id = TroopId(id);
    let assigned = state
        .ring
        .assign_troop_task(&troop_id, &body.task)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
    Ok(Json(AssignTaskResponse {
        assigned_to: assigned,
    }))
}

/// DELETE /api/troops/:id -- disband a troop.
#[instrument(skip(state))]
async fn disband_troop(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let troop_id = TroopId(id);
    state.ring.disband_troop(&troop_id).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/troops/:id/members -- recruit a member.
#[instrument(skip(state, body))]
async fn recruit_member(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<RecruitRequest>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let troop_id = TroopId(id);
    let fighter_id = FighterId(body.fighter_id);
    state
        .ring
        .recruit_to_troop(&troop_id, fighter_id)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /api/troops/:troop_id/members/:fighter_id -- dismiss a member.
#[instrument(skip(state))]
async fn dismiss_member(
    State(state): State<AppState>,
    Path((troop_id, fighter_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let troop_id = TroopId(troop_id);
    let fighter_id = FighterId(fighter_id);
    state
        .ring
        .dismiss_from_troop(&troop_id, &fighter_id)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
    Ok(StatusCode::NO_CONTENT)
}
