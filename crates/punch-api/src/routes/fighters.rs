//! Fighter management endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use punch_types::{FighterId, FighterManifest, FighterStatus, WeightClass};

use crate::AppState;

/// Build the fighter routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/fighters", post(spawn_fighter).get(list_fighters))
        .route("/api/fighters/{id}", get(get_fighter).delete(kill_fighter))
        .route("/api/fighters/{id}/message", post(send_message))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SpawnFighterRequest {
    manifest: FighterManifest,
}

#[derive(Serialize)]
struct SpawnFighterResponse {
    id: FighterId,
    name: String,
}

#[derive(Serialize)]
struct FighterSummary {
    id: FighterId,
    name: String,
    description: String,
    weight_class: WeightClass,
    status: FighterStatus,
}

#[derive(Serialize)]
struct FighterDetail {
    id: FighterId,
    manifest: FighterManifest,
    status: FighterStatus,
}

#[derive(Deserialize)]
struct SendMessageRequest {
    message: String,
}

#[derive(Serialize)]
struct SendMessageResponse {
    response: String,
    tokens_used: u64,
    iterations: usize,
    tool_calls_made: usize,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/fighters — spawn a new fighter.
#[instrument(skip_all)]
async fn spawn_fighter(
    State(state): State<AppState>,
    Json(body): Json<SpawnFighterRequest>,
) -> (StatusCode, Json<SpawnFighterResponse>) {
    let name = body.manifest.name.clone();
    let id = state.ring.spawn_fighter(body.manifest).await;

    (
        StatusCode::CREATED,
        Json(SpawnFighterResponse { id, name }),
    )
}

/// GET /api/fighters — list all fighters.
#[instrument(skip_all)]
async fn list_fighters(State(state): State<AppState>) -> Json<Vec<FighterSummary>> {
    let fighters = state.ring.list_fighters();

    let summaries = fighters
        .into_iter()
        .map(|(id, manifest, status)| FighterSummary {
            id,
            name: manifest.name,
            description: manifest.description,
            weight_class: manifest.weight_class,
            status,
        })
        .collect();

    Json(summaries)
}

/// GET /api/fighters/:id — get fighter details.
#[instrument(skip(state))]
async fn get_fighter(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<FighterDetail>, (StatusCode, Json<ErrorResponse>)> {
    let fighter_id = FighterId(id);

    let entry = state.ring.get_fighter(&fighter_id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("fighter {} not found", id),
            }),
        )
    })?;

    Ok(Json(FighterDetail {
        id: fighter_id,
        manifest: entry.manifest,
        status: entry.status,
    }))
}

/// POST /api/fighters/:id/message — send a message to a fighter.
#[instrument(skip(state, body))]
async fn send_message(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SendMessageRequest>,
) -> Result<Json<SendMessageResponse>, (StatusCode, Json<ErrorResponse>)> {
    let fighter_id = FighterId(id);

    let result = state
        .ring
        .send_message(&fighter_id, body.message)
        .await
        .map_err(|e| {
            let status = match &e {
                punch_types::PunchError::Fighter(_) => StatusCode::NOT_FOUND,
                punch_types::PunchError::RateLimited { .. } => StatusCode::TOO_MANY_REQUESTS,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (
                status,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(SendMessageResponse {
        response: result.response,
        tokens_used: result.usage.total(),
        iterations: result.iterations,
        tool_calls_made: result.tool_calls_made,
    }))
}

/// DELETE /api/fighters/:id — kill a fighter.
#[instrument(skip(state))]
async fn kill_fighter(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> StatusCode {
    let fighter_id = FighterId(id);
    state.ring.kill_fighter(&fighter_id);
    StatusCode::NO_CONTENT
}
