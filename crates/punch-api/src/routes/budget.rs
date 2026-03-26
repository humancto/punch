//! Budget management endpoints — the promoter's purse control panel.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use punch_kernel::{BudgetLimit, BudgetStatus};
use punch_types::FighterId;

use crate::AppState;

/// Build the budget routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/budget", get(get_global_budget))
        .route("/api/budget/global", put(set_global_budget))
        .route(
            "/api/budget/fighters/{id}",
            get(get_fighter_budget).put(set_fighter_budget),
        )
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Deserialize)]
struct SetBudgetRequest {
    #[serde(flatten)]
    limit: BudgetLimit,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/budget — get global budget status.
#[instrument(skip_all)]
async fn get_global_budget(
    State(state): State<AppState>,
) -> Result<Json<BudgetStatus>, (StatusCode, Json<ErrorResponse>)> {
    let status = state
        .ring
        .budget_enforcer()
        .get_global_status()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(status))
}

/// PUT /api/budget/global — set global budget limits.
#[instrument(skip_all)]
async fn set_global_budget(
    State(state): State<AppState>,
    Json(body): Json<SetBudgetRequest>,
) -> StatusCode {
    state.ring.budget_enforcer().set_global_limit(body.limit);

    StatusCode::OK
}

/// GET /api/budget/fighters/:id — get per-fighter budget status.
#[instrument(skip(state))]
async fn get_fighter_budget(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<BudgetStatus>, (StatusCode, Json<ErrorResponse>)> {
    let fighter_id = FighterId(id);

    let status = state
        .ring
        .budget_enforcer()
        .get_fighter_status(&fighter_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(status))
}

/// PUT /api/budget/fighters/:id — set per-fighter budget limits.
#[instrument(skip(state, body))]
async fn set_fighter_budget(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetBudgetRequest>,
) -> StatusCode {
    let fighter_id = FighterId(id);
    state
        .ring
        .budget_enforcer()
        .set_fighter_limit(fighter_id, body.limit);

    StatusCode::OK
}
