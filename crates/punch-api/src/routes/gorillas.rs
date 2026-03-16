//! Gorilla management endpoints.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use tracing::instrument;
use uuid::Uuid;

use punch_types::{GorillaId, GorillaStatus};

use crate::AppState;

/// Build the gorilla routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/gorillas", get(list_gorillas))
        .route("/api/gorillas/{id}/unleash", post(unleash_gorilla))
        .route("/api/gorillas/{id}/cage", post(cage_gorilla))
        .route("/api/gorillas/{id}/status", get(gorilla_status))
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct GorillaSummary {
    id: GorillaId,
    name: String,
    description: String,
    schedule: String,
    status: GorillaStatus,
}

#[derive(Serialize)]
struct GorillaStatusResponse {
    id: GorillaId,
    name: String,
    status: GorillaStatus,
    metrics: GorillaMetricsResponse,
}

#[derive(Serialize)]
struct GorillaMetricsResponse {
    tasks_completed: u64,
    uptime_secs: u64,
    last_rampage: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/gorillas — list all gorillas.
#[instrument(skip_all)]
async fn list_gorillas(State(state): State<AppState>) -> Json<Vec<GorillaSummary>> {
    let gorillas = state.ring.list_gorillas().await;

    let summaries = gorillas
        .into_iter()
        .map(|(id, manifest, status, _metrics)| GorillaSummary {
            id,
            name: manifest.name,
            description: manifest.description,
            schedule: manifest.schedule,
            status,
        })
        .collect();

    Json(summaries)
}

/// POST /api/gorillas/:id/unleash — start a gorilla.
#[instrument(skip(state))]
async fn unleash_gorilla(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let gorilla_id = GorillaId(id);

    state.ring.unleash_gorilla(&gorilla_id).await.map_err(|e| {
        let status = match &e {
            punch_types::PunchError::Gorilla(msg) if msg.contains("not found") => {
                StatusCode::NOT_FOUND
            }
            punch_types::PunchError::Gorilla(msg) if msg.contains("already active") => {
                StatusCode::CONFLICT
            }
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::OK)
}

/// POST /api/gorillas/:id/cage — stop a gorilla.
#[instrument(skip(state))]
async fn cage_gorilla(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let gorilla_id = GorillaId(id);

    state.ring.cage_gorilla(&gorilla_id).await.map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::OK)
}

/// GET /api/gorillas/:id/status — get gorilla metrics.
#[instrument(skip(state))]
async fn gorilla_status(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<GorillaStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    let gorilla_id = GorillaId(id);

    // We need to find this gorilla in the list since Ring doesn't expose
    // a single-gorilla lookup with metrics yet.
    let gorillas = state.ring.list_gorillas().await;

    let (_, manifest, status, metrics) = gorillas
        .into_iter()
        .find(|(gid, _, _, _)| *gid == gorilla_id)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("gorilla {} not found", id),
                }),
            )
        })?;

    Ok(Json(GorillaStatusResponse {
        id: gorilla_id,
        name: manifest.name,
        status,
        metrics: GorillaMetricsResponse {
            tasks_completed: metrics.tasks_completed,
            uptime_secs: metrics.uptime_secs,
            last_rampage: metrics.last_rampage,
        },
    }))
}
