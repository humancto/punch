//! Creed management endpoints — the consciousness layer API.
//!
//! Endpoints:
//!   GET    /api/creeds              — List all creeds
//!   POST   /api/creeds              — Create a new creed
//!   GET    /api/creeds/:name        — Get a creed by fighter name
//!   PUT    /api/creeds/:name        — Update a creed
//!   DELETE /api/creeds/:name        — Delete a creed
//!   POST   /api/creeds/:name/learn  — Add a learned behavior
//!   GET    /api/creeds/:name/render — Render creed as system prompt section

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use punch_types::{Creed, InteractionStyle};
use serde::Deserialize;
use tracing::instrument;

use crate::AppState;

/// Build the creed routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/creeds", post(create_creed).get(list_creeds))
        .route(
            "/api/creeds/{name}",
            get(get_creed).put(update_creed).delete(delete_creed),
        )
        .route("/api/creeds/{name}/learn", post(learn))
        .route("/api/creeds/{name}/render", get(render_creed))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CreateCreedRequest {
    /// Fighter name this creed belongs to.
    fighter_name: String,
    /// Identity text — who this fighter IS.
    identity: String,
    /// Personality traits as key-value pairs.
    #[serde(default)]
    personality: HashMap<String, f64>,
    /// Core directives.
    #[serde(default)]
    directives: Vec<String>,
    /// Interaction style.
    #[serde(default)]
    interaction_style: Option<InteractionStyleRequest>,
}

#[derive(Debug, Deserialize)]
struct InteractionStyleRequest {
    #[serde(default)]
    verbosity: Option<String>,
    #[serde(default)]
    tone: Option<String>,
    #[serde(default)]
    uses_metaphors: Option<bool>,
    #[serde(default)]
    proactive: Option<bool>,
    #[serde(default)]
    notes: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct LearnRequest {
    observation: String,
    #[serde(default = "default_confidence")]
    confidence: f64,
}

fn default_confidence() -> f64 {
    0.8
}

#[derive(serde::Serialize)]
struct ErrorResponse {
    error: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn error_response(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (status, Json(ErrorResponse { error: msg.into() }))
}

fn apply_interaction_style(style: &mut InteractionStyle, req: &InteractionStyleRequest) {
    if let Some(ref v) = req.verbosity {
        style.verbosity = v.clone();
    }
    if let Some(ref t) = req.tone {
        style.tone = t.clone();
    }
    if let Some(m) = req.uses_metaphors {
        style.uses_metaphors = m;
    }
    if let Some(p) = req.proactive {
        style.proactive = p;
    }
    if let Some(ref n) = req.notes {
        style.notes = n.clone();
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/creeds — list all creeds.
#[instrument(skip_all)]
async fn list_creeds(
    State(state): State<AppState>,
) -> Result<Json<Vec<Creed>>, (StatusCode, Json<ErrorResponse>)> {
    let memory = state.ring.memory();
    let creeds = memory.list_creeds().await.map_err(|e| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to list creeds: {e}"),
        )
    })?;
    Ok(Json(creeds))
}

/// POST /api/creeds — create a new creed.
#[instrument(skip_all)]
async fn create_creed(
    State(state): State<AppState>,
    Json(req): Json<CreateCreedRequest>,
) -> Result<(StatusCode, Json<Creed>), (StatusCode, Json<ErrorResponse>)> {
    let memory = state.ring.memory();

    // Check if a creed already exists for this fighter name.
    if let Some(_existing) = memory
        .load_creed_by_name(&req.fighter_name)
        .await
        .map_err(|e| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to check existing creed: {e}"),
            )
        })?
    {
        return Err(error_response(
            StatusCode::CONFLICT,
            format!("creed already exists for fighter '{}'", req.fighter_name),
        ));
    }

    // Build the creed.
    let mut creed = Creed::new(&req.fighter_name).with_identity(&req.identity);

    for (name, value) in &req.personality {
        creed = creed.with_trait(name, *value);
    }
    for directive in &req.directives {
        creed = creed.with_directive(directive);
    }

    // If the fighter exists in the Ring, apply self-awareness from its manifest.
    let fighters = state.ring.list_fighters();
    if let Some((_id, manifest, _status)) = fighters
        .iter()
        .find(|(_id, m, _s)| m.name.eq_ignore_ascii_case(&req.fighter_name))
    {
        creed = creed.with_self_awareness(manifest);
    }

    // Apply interaction style if provided.
    if let Some(ref style_req) = req.interaction_style {
        apply_interaction_style(&mut creed.interaction_style, style_req);
    }

    memory.save_creed(&creed).await.map_err(|e| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to save creed: {e}"),
        )
    })?;

    Ok((StatusCode::CREATED, Json(creed)))
}

/// GET /api/creeds/:name — get a creed by fighter name.
#[instrument(skip(state))]
async fn get_creed(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<Creed>, (StatusCode, Json<ErrorResponse>)> {
    let memory = state.ring.memory();
    let creed = memory.load_creed_by_name(&name).await.map_err(|e| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to load creed: {e}"),
        )
    })?;

    match creed {
        Some(c) => Ok(Json(c)),
        None => Err(error_response(
            StatusCode::NOT_FOUND,
            format!("no creed found for fighter '{name}'"),
        )),
    }
}

/// PUT /api/creeds/:name — update an existing creed.
#[instrument(skip(state, req))]
async fn update_creed(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<CreateCreedRequest>,
) -> Result<Json<Creed>, (StatusCode, Json<ErrorResponse>)> {
    let memory = state.ring.memory();

    let mut creed = memory
        .load_creed_by_name(&name)
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
                format!("no creed found for fighter '{name}'"),
            )
        })?;

    // Merge updates.
    creed.identity = req.identity;

    for (trait_name, value) in &req.personality {
        creed
            .personality
            .insert(trait_name.clone(), value.clamp(0.0, 1.0));
    }

    for directive in &req.directives {
        if !creed.directives.contains(directive) {
            creed.directives.push(directive.clone());
        }
    }

    if let Some(ref style_req) = req.interaction_style {
        apply_interaction_style(&mut creed.interaction_style, style_req);
    }

    creed.version += 1;
    creed.updated_at = chrono::Utc::now();

    memory.save_creed(&creed).await.map_err(|e| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to save creed: {e}"),
        )
    })?;

    Ok(Json(creed))
}

/// DELETE /api/creeds/:name — delete a creed.
#[instrument(skip(state))]
async fn delete_creed(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let memory = state.ring.memory();
    memory.delete_creed(&name).await.map_err(|e| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to delete creed: {e}"),
        )
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/creeds/:name/learn — add a learned behavior.
#[instrument(skip(state, req))]
async fn learn(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<LearnRequest>,
) -> Result<Json<Creed>, (StatusCode, Json<ErrorResponse>)> {
    let memory = state.ring.memory();

    let mut creed = memory
        .load_creed_by_name(&name)
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
                format!("no creed found for fighter '{name}'"),
            )
        })?;

    creed.learn(&req.observation, req.confidence);

    memory.save_creed(&creed).await.map_err(|e| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to save creed: {e}"),
        )
    })?;

    Ok(Json(creed))
}

/// GET /api/creeds/:name/render — render creed as a system prompt section.
#[instrument(skip(state))]
async fn render_creed(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let memory = state.ring.memory();

    let creed = memory
        .load_creed_by_name(&name)
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
                format!("no creed found for fighter '{name}'"),
            )
        })?;

    let rendered = creed.render();

    Ok((
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        rendered,
    )
        .into_response())
}
