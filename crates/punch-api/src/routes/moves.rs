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
        .route("/api/moves/installed", get(list_installed))
        .route("/api/moves/marketplace", get(search_marketplace))
        .route("/api/moves/sync", post(sync_index))
        .route("/api/moves/{name}", get(get_move).delete(uninstall_move))
        .route("/api/moves/{name}/install", post(install_move))
        .route("/api/moves/{name}/report", post(report_move))
        .route("/api/moves/scan/{name}", get(scan_move))
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ListQuery {
    q: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MarketplaceQuery {
    q: Option<String>,
    category: Option<String>,
    tag: Option<String>,
    page: Option<usize>,
    per_page: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct ReportRequest {
    reason: String,
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

#[derive(Serialize)]
struct InstalledSummary {
    name: String,
    version: String,
    enabled: bool,
    installed_at: String,
}

#[derive(Serialize)]
struct MarketplaceListing {
    name: String,
    description: String,
    version: String,
    author: String,
    category: String,
    tags: Vec<String>,
    install_count: u64,
    rating: f64,
}

#[derive(Serialize)]
struct SyncResponse {
    message: String,
}

#[derive(Serialize)]
struct ReportResponse {
    message: String,
}

#[derive(Serialize)]
struct ScanResponse {
    name: String,
    verdict: String,
    findings: Vec<ScanFindingInfo>,
}

#[derive(Serialize)]
struct ScanFindingInfo {
    severity: String,
    line: usize,
    description: String,
    pattern: String,
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

    let listing = marketplace.find_by_name(&name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("move '{}' not found", name),
            }),
        )
    })?;

    let detail = MoveDetail {
        name: listing.name.clone(),
        move_type: source_type(&listing),
        description: listing.description.clone(),
        version: listing.version.clone(),
        parameters: extract_parameters(&listing),
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
    let listing = marketplace.find_by_name(&name).ok_or_else(|| {
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

/// DELETE /api/moves/:name — uninstall a move by name.
#[instrument(skip(state))]
async fn uninstall_move(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<InstallResponse>, (StatusCode, Json<ErrorResponse>)> {
    let marketplace = state.ring.marketplace();

    let listing = marketplace.find_by_name(&name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("move '{}' not found", name),
            }),
        )
    })?;

    marketplace.uninstall(&listing.id).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(InstallResponse {
        name: listing.name.clone(),
        message: format!("Uninstalled {}", listing.name),
    }))
}

/// GET /api/moves/installed — list all installed moves.
#[instrument(skip_all)]
async fn list_installed(State(state): State<AppState>) -> Json<Vec<InstalledSummary>> {
    let marketplace = state.ring.marketplace();
    let installed = marketplace.installed_skills();

    let summaries: Vec<InstalledSummary> = installed
        .iter()
        .map(|s| {
            let name = marketplace
                .get(&s.skill_id)
                .map(|l| l.name)
                .unwrap_or_else(|| s.skill_id.to_string());
            let version = marketplace
                .get(&s.skill_id)
                .map(|l| l.version)
                .unwrap_or_else(|| "unknown".to_string());
            InstalledSummary {
                name,
                version,
                enabled: s.enabled,
                installed_at: s.installed_at.to_rfc3339(),
            }
        })
        .collect();

    Json(summaries)
}

/// GET /api/moves/marketplace — search the remote index.
#[instrument(skip_all)]
async fn search_marketplace(
    State(state): State<AppState>,
    Query(query): Query<MarketplaceQuery>,
) -> Json<Vec<MarketplaceListing>> {
    let marketplace = state.ring.marketplace();

    let mut listings: Vec<SkillListing> = match query.q.as_deref() {
        Some(q) if !q.is_empty() => marketplace.search(q),
        _ => marketplace.list_all(),
    };

    // Filter by category
    if let Some(ref cat) = query.category {
        let cat_lower = cat.to_lowercase();
        listings.retain(|l| l.category.to_lowercase() == cat_lower);
    }

    // Filter by tag
    if let Some(ref tag) = query.tag {
        let tag_lower = tag.to_lowercase();
        listings.retain(|l| l.tags.iter().any(|t| t.to_lowercase() == tag_lower));
    }

    // Pagination
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).min(100);
    let start = (page - 1) * per_page;

    let paginated: Vec<MarketplaceListing> = listings
        .into_iter()
        .skip(start)
        .take(per_page)
        .map(|l| MarketplaceListing {
            name: l.name,
            description: l.description,
            version: l.version,
            author: l.author,
            category: l.category,
            tags: l.tags,
            install_count: l.install_count,
            rating: l.rating,
        })
        .collect();

    Json(paginated)
}

/// POST /api/moves/sync — trigger index synchronization.
#[instrument(skip_all)]
async fn sync_index(
    State(_state): State<AppState>,
) -> Result<Json<SyncResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Sync using the default index location
    let punch_home = std::env::var("PUNCH_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".punch")))
        .unwrap_or_else(|_| std::path::PathBuf::from(".punch"));

    let client = punch_skills::IndexClient::with_defaults(&punch_home);
    client.sync().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("index sync failed: {}", e),
            }),
        )
    })?;

    Ok(Json(SyncResponse {
        message: "Index synchronized successfully".to_string(),
    }))
}

/// POST /api/moves/:name/report — report a problematic move.
#[instrument(skip(state))]
async fn report_move(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<ReportRequest>,
) -> Result<Json<ReportResponse>, (StatusCode, Json<ErrorResponse>)> {
    let marketplace = state.ring.marketplace();

    // Verify the move exists
    marketplace.find_by_name(&name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("move '{}' not found", name),
            }),
        )
    })?;

    tracing::warn!(
        move_name = %name,
        reason = %body.reason,
        "move reported by user"
    );

    Ok(Json(ReportResponse {
        message: format!("Report for '{}' submitted. Reason: {}", name, body.reason),
    }))
}

/// GET /api/moves/scan/:name — run security scan on an installed move.
#[instrument(skip(state))]
async fn scan_move(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<ScanResponse>, (StatusCode, Json<ErrorResponse>)> {
    let marketplace = state.ring.marketplace();

    let listing = marketplace.find_by_name(&name).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("move '{}' not found", name),
            }),
        )
    })?;

    // Build scannable content from the listing
    let content = format!(
        "---\nname: {}\nversion: {}\ndescription: {}\nauthor: {}\n---\n\n{}",
        listing.name, listing.version, listing.description, listing.author, listing.description
    );

    let scanner = punch_skills::SkillScanner::new();
    let verdict = scanner.scan(&content);

    let (verdict_str, findings_list) = match &verdict {
        punch_skills::ScanVerdict::Clean => ("clean".to_string(), vec![]),
        punch_skills::ScanVerdict::Warning(findings) => (
            format!("warning ({} finding(s))", findings.len()),
            findings
                .iter()
                .map(|f| ScanFindingInfo {
                    severity: f.severity.clone(),
                    line: f.line,
                    description: f.description.clone(),
                    pattern: f.pattern.clone(),
                })
                .collect(),
        ),
        punch_skills::ScanVerdict::Rejected(findings) => (
            format!("rejected ({} finding(s))", findings.len()),
            findings
                .iter()
                .map(|f| ScanFindingInfo {
                    severity: f.severity.clone(),
                    line: f.line,
                    description: f.description.clone(),
                    pattern: f.pattern.clone(),
                })
                .collect(),
        ),
    };

    Ok(Json(ScanResponse {
        name: listing.name,
        verdict: verdict_str,
        findings: findings_list,
    }))
}
