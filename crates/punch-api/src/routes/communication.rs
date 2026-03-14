//! Inter-agent communication endpoints.
//!
//! These endpoints enable fighter-to-fighter messaging and multi-turn
//! conversations between agents, bringing the Creed system alive with
//! agents that are aware of each other, communicating and collaborating.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::{info, instrument};
use uuid::Uuid;

use punch_types::FighterId;

use crate::AppState;

/// Build the inter-agent communication routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/fighters/{source_id}/message-to/{target_id}",
            post(fighter_to_fighter),
        )
        .route("/api/fighters/conversation", post(conversation))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct FighterToFighterRequest {
    message: String,
}

#[derive(Serialize)]
struct FighterToFighterResponse {
    source: String,
    target: String,
    response: String,
    usage: UsageInfo,
}

#[derive(Deserialize)]
struct ConversationRequest {
    /// Name of the first fighter.
    fighter_a: String,
    /// Name of the second fighter.
    fighter_b: String,
    /// The initial topic or prompt to start the conversation.
    topic: String,
    /// Number of back-and-forth turns (capped at 10).
    #[serde(default = "default_turns")]
    turns: usize,
}

fn default_turns() -> usize {
    3
}

#[derive(Serialize)]
struct ConversationResponse {
    conversation: Vec<ConversationTurn>,
    total_tokens: u64,
}

#[derive(Serialize)]
struct ConversationTurn {
    speaker: String,
    message: String,
}

#[derive(Serialize)]
struct UsageInfo {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/fighters/:source_id/message-to/:target_id
///
/// Send a message from one fighter to another. The source fighter's message
/// becomes the target fighter's input (enriched with source context).
/// The target processes it through its own fighter loop with its own creed.
#[instrument(skip(state, body))]
async fn fighter_to_fighter(
    State(state): State<AppState>,
    Path((source_id, target_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<FighterToFighterRequest>,
) -> Result<Json<FighterToFighterResponse>, (StatusCode, Json<ErrorResponse>)> {
    let source_fid = FighterId(source_id);
    let target_fid = FighterId(target_id);

    // Get fighter names for the response.
    let source_name = state
        .ring
        .get_fighter(&source_fid)
        .map(|e| e.manifest.name.clone())
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("source fighter {} not found", source_id),
                }),
            )
        })?;

    let target_name = state
        .ring
        .get_fighter(&target_fid)
        .map(|e| e.manifest.name.clone())
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("target fighter {} not found", target_id),
                }),
            )
        })?;

    let result = state
        .ring
        .fighter_to_fighter(&source_fid, &target_fid, body.message)
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

    // Update relationship tracking in both creeds.
    state
        .ring
        .update_fighter_relationships(&source_name, &target_name)
        .await;

    info!(
        source = %source_name,
        target = %target_name,
        tokens = result.usage.total(),
        "fighter-to-fighter message delivered"
    );

    Ok(Json(FighterToFighterResponse {
        source: source_name,
        target: target_name,
        response: result.response,
        usage: UsageInfo {
            input_tokens: result.usage.input_tokens,
            output_tokens: result.usage.output_tokens,
            total_tokens: result.usage.total(),
        },
    }))
}

/// POST /api/fighters/conversation
///
/// Run a multi-turn conversation between two fighters. The topic is sent
/// to fighter_a first, then each response is forwarded to the other fighter
/// for the specified number of turns.
#[instrument(skip(state, body))]
async fn conversation(
    State(state): State<AppState>,
    Json(body): Json<ConversationRequest>,
) -> Result<Json<ConversationResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Cap turns at 10 to prevent runaway token usage.
    let turns = body.turns.min(10);
    if turns == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "turns must be at least 1".to_string(),
            }),
        ));
    }

    // Look up fighters by name.
    let (id_a, _) = state
        .ring
        .find_fighter_by_name_sync(&body.fighter_a)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("fighter '{}' not found", body.fighter_a),
                }),
            )
        })?;

    let (id_b, _) = state
        .ring
        .find_fighter_by_name_sync(&body.fighter_b)
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: format!("fighter '{}' not found", body.fighter_b),
                }),
            )
        })?;

    let name_a = body.fighter_a.clone();
    let name_b = body.fighter_b.clone();

    let mut conversation = Vec::new();
    let mut total_tokens: u64 = 0;

    // First turn: send the topic to fighter A.
    let mut current_message = body.topic.clone();
    let mut current_speaker_is_a = true;

    for turn in 0..(turns * 2) {
        let (source_id, target_id, speaker_name) = if current_speaker_is_a {
            // Sending to A — in the first turn this is the topic, in later turns
            // it's B's response being sent to A.
            if turn == 0 {
                // Initial topic goes to A as a direct message (no source fighter).
                let result = state
                    .ring
                    .send_message(&id_a, current_message.clone())
                    .await
                    .map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(ErrorResponse {
                                error: format!("fighter_a error: {}", e),
                            }),
                        )
                    })?;

                total_tokens += result.usage.total();
                conversation.push(ConversationTurn {
                    speaker: name_a.clone(),
                    message: result.response.clone(),
                });
                current_message = result.response;
                current_speaker_is_a = false;
                continue;
            }
            (&id_b, &id_a, &name_a)
        } else {
            (&id_a, &id_b, &name_b)
        };

        let result = state
            .ring
            .fighter_to_fighter(source_id, target_id, current_message)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("{} error on turn {}: {}", speaker_name, turn, e),
                    }),
                )
            })?;

        total_tokens += result.usage.total();
        conversation.push(ConversationTurn {
            speaker: speaker_name.clone(),
            message: result.response.clone(),
        });
        current_message = result.response;
        current_speaker_is_a = !current_speaker_is_a;
    }

    // Update relationship tracking after the conversation.
    state
        .ring
        .update_fighter_relationships(&name_a, &name_b)
        .await;

    info!(
        fighter_a = %name_a,
        fighter_b = %name_b,
        turns = conversation.len(),
        total_tokens,
        "conversation completed"
    );

    Ok(Json(ConversationResponse {
        conversation,
        total_tokens,
    }))
}
