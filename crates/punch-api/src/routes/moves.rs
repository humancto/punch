//! Moves (skills/tools) marketplace endpoints.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::instrument;

use punch_skills::SkillListing;

use crate::AppState;

/// Build the moves routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/moves", get(list_moves))
        .route("/api/moves/{name}", get(get_move))
        .route("/api/moves/{name}/install", post(install_move))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
}

#[derive(Serialize)]
struct MoveSummary {
    name: String,
    #[serde(rename = "type")]
    move_type: String,
    description: String,
}

#[derive(Serialize)]
struct MoveDetail {
    name: String,
    #[serde(rename = "type")]
    move_type: String,
    description: String,
    version: String,
    parameters: Vec<ParameterInfo>,
}

#[derive(Serialize)]
struct ParameterInfo {
    name: String,
    #[serde(rename = "type")]
    param_type: String,
    required: bool,
}

#[derive(Serialize)]
struct InstallResponse {
    name: String,
    message: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Derive the move type string from a skill listing's source.
fn source_type(listing: &SkillListing) -> String {
    match &listing.source {
        punch_skills::SkillSource::Builtin => "built-in".to_string(),
        punch_skills::SkillSource::Local(_) => "local".to_string(),
        punch_skills::SkillSource::Remote(_) => "remote".to_string(),
        punch_skills::SkillSource::Plugin(_) => "plugin".to_string(),
        punch_skills::SkillSource::Marketplace { .. } => "marketplace".to_string(),
    }
}

/// Convert a skill listing to a summary for list/search responses.
fn to_summary(listing: &SkillListing) -> MoveSummary {
    MoveSummary {
        name: listing.name.clone(),
        move_type: source_type(listing),
        description: listing.description.clone(),
    }
}

/// Extract parameter info from a skill listing's tool definitions.
fn extract_parameters(listing: &SkillListing) -> Vec<ParameterInfo> {
    let mut params = Vec::new();
    for tool in &listing.tool_definitions {
        if let Some(props) = tool
            .input_schema
            .get("properties")
            .and_then(|p| p.as_object())
        {
            let required_fields: Vec<String> = tool
                .input_schema
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            for (name, schema) in props {
                let param_type = schema
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("string")
                    .to_string();
                params.push(ParameterInfo {
                    name: name.clone(),
                    param_type,
                    required: required_fields.contains(name),
                });
            }
        }
    }
    params
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/moves — list all moves, or search if `?q=` is provided.
#[instrument(skip_all)]
async fn list_moves(
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Json<Vec<MoveSummary>> {
    let marketplace = state.ring.marketplace();

    let listings: Vec<SkillListing> = match query.q.as_deref() {
        Some(q) if !q.is_empty() => marketplace.search(q),
        _ => marketplace.search(""),
    };

    let summaries: Vec<MoveSummary> = listings.iter().map(to_summary).collect();
    Json(summaries)
}

/// GET /api/moves/:name — get detailed info about a specific move.
#[instrument(skip(state))]
async fn get_move(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<MoveDetail>, (StatusCode, Json<ErrorResponse>)> {
    let marketplace = state.ring.marketplace();

    // Search for the skill by name (exact match, case-insensitive).
    let listings = marketplace.search(&name);
    let listing = listings
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(&name))
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("move '{}' not found", name),
                }),
            )
        })?;

    let detail = MoveDetail {
        name: listing.name.clone(),
        move_type: source_type(listing),
        description: listing.description.clone(),
        version: listing.version.clone(),
        parameters: extract_parameters(listing),
    };

    Ok(Json(detail))
}

/// POST /api/moves/:name/install — install a move by name.
#[instrument(skip(state))]
async fn install_move(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<InstallResponse>, (StatusCode, Json<ErrorResponse>)> {
    let marketplace = state.ring.marketplace();

    // Find the skill by name.
    let listings = marketplace.search(&name);
    let listing = listings
        .iter()
        .find(|l| l.name.eq_ignore_ascii_case(&name))
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("move '{}' not found in marketplace", name),
                }),
            )
        })?;

    let skill_id = listing.id;
    marketplace.install(&skill_id).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(InstallResponse {
        name: listing.name.clone(),
        message: format!(
            "Installed {} v{} ({} tool(s) added)",
            listing.name,
            listing.version,
            listing.tool_definitions.len()
        ),
    }))
}
