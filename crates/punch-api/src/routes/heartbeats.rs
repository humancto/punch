//! Heartbeat management endpoints — proactive task scheduling via the creed system.
//!
//! Endpoints:
//!   GET    /api/heartbeats        — List all heartbeat tasks across all fighters
//!   POST   /api/heartbeats        — Add a heartbeat task to a fighter's creed
//!   DELETE /api/heartbeats/:index — Remove a heartbeat task by index

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use punch_types::HeartbeatTask;
use serde::{Deserialize, Serialize};
use tracing::instrument;

use crate::AppState;

/// Build the heartbeat routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/heartbeats", get(list_heartbeats).post(add_heartbeat))
        .route("/api/heartbeats/{index}", delete(remove_heartbeat))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct HeartbeatEntry {
    fighter_name: String,
    heartbeat: Vec<HeartbeatTask>,
}

#[derive(Debug, Deserialize)]
struct AddHeartbeatRequest {
    task: String,
    #[serde(default = "default_cadence")]
    cadence: String,
    /// Fighter name. If omitted, uses the first fighter with a creed.
    fighter_name: Option<String>,
}

fn default_cadence() -> String {
    "hourly".to_string()
}

#[derive(Debug, Deserialize)]
struct RemoveQuery {
    /// Fighter name to remove from. If omitted, uses the first fighter with a creed.
    fighter_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn error_response(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (status, Json(ErrorResponse { error: msg.into() }))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/heartbeats — list all heartbeat tasks across all fighters.
#[instrument(skip_all)]
async fn list_heartbeats(
    State(state): State<AppState>,
) -> Result<Json<Vec<HeartbeatEntry>>, (StatusCode, Json<ErrorResponse>)> {
    let memory = state.ring.memory();
    let creeds = memory.list_creeds().await.map_err(|e| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to list creeds: {e}"),
        )
    })?;

    let entries: Vec<HeartbeatEntry> = creeds
        .into_iter()
        .filter(|c| !c.heartbeat.is_empty())
        .map(|c| HeartbeatEntry {
            fighter_name: c.fighter_name,
            heartbeat: c.heartbeat,
        })
        .collect();

    Ok(Json(entries))
}

/// POST /api/heartbeats — add a heartbeat task to a fighter's creed.
#[instrument(skip_all)]
async fn add_heartbeat(
    State(state): State<AppState>,
    Json(req): Json<AddHeartbeatRequest>,
) -> Result<(StatusCode, Json<HeartbeatEntry>), (StatusCode, Json<ErrorResponse>)> {
    // Validate cadence
    let valid_cadences = ["every_bout", "hourly", "daily", "on_wake"];
    if !valid_cadences.contains(&req.cadence.as_str()) {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            format!(
                "invalid cadence '{}'. Must be one of: {}",
                req.cadence,
                valid_cadences.join(", ")
            ),
        ));
    }

    let memory = state.ring.memory();

    // Find the target creed
    let fighter_name = match req.fighter_name {
        Some(ref name) => name.clone(),
        None => {
            // Use the first creed we find
            let creeds = memory.list_creeds().await.map_err(|e| {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to list creeds: {e}"),
                )
            })?;
            creeds
                .first()
                .map(|c| c.fighter_name.clone())
                .ok_or_else(|| {
                    error_response(
                        StatusCode::NOT_FOUND,
                        "no creeds found — spawn a fighter first",
                    )
                })?
        }
    };

    let mut creed = memory
        .load_creed_by_name(&fighter_name)
        .await
        .map_err(|e| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to load creed: {e}"),
            )
        })?
        .ok_or_else(|| {
            error_response(
                StatusCode::NOT_FOUND,
                format!("no creed found for fighter '{}'", fighter_name),
            )
        })?;

    // Add the heartbeat task
    creed.heartbeat.push(HeartbeatTask {
        task: req.task,
        cadence: req.cadence,
        active: true,
        execution_count: 0,
        last_checked: None,
    });

    memory.save_creed(&creed).await.map_err(|e| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to save creed: {e}"),
        )
    })?;

    Ok((
        StatusCode::CREATED,
        Json(HeartbeatEntry {
            fighter_name: creed.fighter_name,
            heartbeat: creed.heartbeat,
        }),
    ))
}

/// DELETE /api/heartbeats/:index — remove a heartbeat task by index.
#[instrument(skip_all)]
async fn remove_heartbeat(
    State(state): State<AppState>,
    Path(index): Path<usize>,
    Query(query): Query<RemoveQuery>,
) -> Result<Json<HeartbeatEntry>, (StatusCode, Json<ErrorResponse>)> {
    let memory = state.ring.memory();

    // Find the target creed
    let fighter_name = match query.fighter_name {
        Some(ref name) => name.clone(),
        None => {
            let creeds = memory.list_creeds().await.map_err(|e| {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to list creeds: {e}"),
                )
            })?;
            creeds
                .first()
                .map(|c| c.fighter_name.clone())
                .ok_or_else(|| {
                    error_response(
                        StatusCode::NOT_FOUND,
                        "no creeds found — spawn a fighter first",
                    )
                })?
        }
    };

    let mut creed = memory
        .load_creed_by_name(&fighter_name)
        .await
        .map_err(|e| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to load creed: {e}"),
            )
        })?
        .ok_or_else(|| {
            error_response(
                StatusCode::NOT_FOUND,
                format!("no creed found for fighter '{}'", fighter_name),
            )
        })?;

    if index >= creed.heartbeat.len() {
        return Err(error_response(
            StatusCode::BAD_REQUEST,
            format!(
                "index {} out of range (fighter '{}' has {} heartbeat tasks)",
                index,
                fighter_name,
                creed.heartbeat.len()
            ),
        ));
    }

    creed.heartbeat.remove(index);

    memory.save_creed(&creed).await.map_err(|e| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to save creed: {e}"),
        )
    })?;

    Ok(Json(HeartbeatEntry {
        fighter_name: creed.fighter_name,
        heartbeat: creed.heartbeat,
    }))
}
