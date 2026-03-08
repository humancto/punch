//! Health and status endpoints.

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use tracing::instrument;

use crate::AppState;

/// Build the health routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health_check))
        .route("/api/status", get(system_status))
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct SystemStatus {
    status: &'static str,
    fighter_count: usize,
    gorilla_count: usize,
    uptime_secs: i64,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /health — simple health check.
#[instrument(skip_all)]
async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

/// GET /api/status — detailed system status.
#[instrument(skip_all)]
async fn system_status(State(state): State<AppState>) -> Json<SystemStatus> {
    let fighters = state.ring.list_fighters();
    let gorillas = state.ring.list_gorillas().await;
    let uptime = chrono::Utc::now()
        .signed_duration_since(state.started_at)
        .num_seconds();

    Json(SystemStatus {
        status: "ok",
        fighter_count: fighters.len(),
        gorilla_count: gorillas.len(),
        uptime_secs: uptime,
    })
}
