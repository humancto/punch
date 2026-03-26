//! Token usage statistics endpoints — the promoter's ledger.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use punch_kernel::SpendPeriod;
use punch_types::FighterId;

use crate::AppState;

/// Build the stats routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/stats", get(get_global_stats))
        .route("/api/stats/fighters/{id}", get(get_fighter_stats))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct StatsQuery {
    /// Time period: "hour", "day", or "month" (default: "day").
    #[serde(default = "default_period")]
    period: String,
}

fn default_period() -> String {
    "day".to_string()
}

fn parse_period(s: &str) -> Result<SpendPeriod, String> {
    match s {
        "hour" => Ok(SpendPeriod::Hour),
        "day" => Ok(SpendPeriod::Day),
        "month" => Ok(SpendPeriod::Month),
        other => Err(format!(
            "invalid period: {other} (expected hour, day, or month)"
        )),
    }
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize)]
struct ModelBreakdown {
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    request_count: u64,
}

#[derive(Serialize)]
struct FighterBreakdown {
    fighter_id: String,
    fighter_name: String,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
    request_count: u64,
}

#[derive(Serialize)]
struct GlobalStatsResponse {
    period: String,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cost_usd: f64,
    total_requests: u64,
    by_model: Vec<ModelBreakdown>,
    by_fighter: Vec<FighterBreakdown>,
}

#[derive(Serialize)]
struct FighterStatsResponse {
    fighter_id: String,
    fighter_name: String,
    period: String,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_cost_usd: f64,
    total_requests: u64,
    by_model: Vec<ModelBreakdown>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/stats — global usage stats across all fighters.
#[instrument(skip_all)]
async fn get_global_stats(
    State(state): State<AppState>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<GlobalStatsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let period = parse_period(&query.period)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?;

    let metering = state.ring.metering();

    let summary = metering.get_total_summary(period).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    let model_breakdown = metering
        .get_total_model_breakdown(period)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    let fighter_breakdown = metering.get_fighter_breakdown(period).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    // Resolve fighter names from the Ring.
    let fighters = state.ring.list_fighters();
    let fighter_name_map: std::collections::HashMap<FighterId, String> = fighters
        .iter()
        .map(|(id, manifest, _)| (*id, manifest.name.clone()))
        .collect();

    let by_fighter: Vec<FighterBreakdown> = fighter_breakdown
        .into_iter()
        .map(|fb| FighterBreakdown {
            fighter_id: fb.fighter_id.to_string(),
            fighter_name: fighter_name_map
                .get(&fb.fighter_id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string()),
            input_tokens: fb.input_tokens,
            output_tokens: fb.output_tokens,
            cost_usd: fb.cost_usd,
            request_count: fb.request_count,
        })
        .collect();

    let by_model: Vec<ModelBreakdown> = model_breakdown
        .into_iter()
        .map(|mb| ModelBreakdown {
            model: mb.model,
            input_tokens: mb.input_tokens,
            output_tokens: mb.output_tokens,
            cost_usd: mb.cost_usd,
            request_count: mb.request_count,
        })
        .collect();

    Ok(Json(GlobalStatsResponse {
        period: query.period,
        total_input_tokens: summary.total_input_tokens,
        total_output_tokens: summary.total_output_tokens,
        total_cost_usd: summary.total_cost_usd,
        total_requests: summary.event_count,
        by_model,
        by_fighter,
    }))
}

/// GET /api/stats/fighters/:id — per-fighter usage stats.
#[instrument(skip(state))]
async fn get_fighter_stats(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(query): Query<StatsQuery>,
) -> Result<Json<FighterStatsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let period = parse_period(&query.period)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?;

    let fighter_id = FighterId(id);
    let metering = state.ring.metering();

    // Look up fighter name.
    let fighters = state.ring.list_fighters();
    let fighter_name = fighters
        .iter()
        .find(|(fid, _, _)| *fid == fighter_id)
        .map(|(_, m, _)| m.name.clone())
        .unwrap_or_else(|| "unknown".to_string());

    let summary = metering
        .get_fighter_summary(&fighter_id, period)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    let model_breakdown = metering
        .get_model_breakdown(&fighter_id, period)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    let by_model: Vec<ModelBreakdown> = model_breakdown
        .into_iter()
        .map(|mb| ModelBreakdown {
            model: mb.model,
            input_tokens: mb.input_tokens,
            output_tokens: mb.output_tokens,
            cost_usd: mb.cost_usd,
            request_count: mb.request_count,
        })
        .collect();

    Ok(Json(FighterStatsResponse {
        fighter_id: fighter_id.to_string(),
        fighter_name,
        period: query.period,
        total_input_tokens: summary.total_input_tokens,
        total_output_tokens: summary.total_output_tokens,
        total_cost_usd: summary.total_cost_usd,
        total_requests: summary.event_count,
        by_model,
    }))
}
