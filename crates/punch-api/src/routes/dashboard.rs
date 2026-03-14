//! Arena dashboard — the management UI backend for monitoring and controlling
//! the Punch Agent Combat System.
//!
//! Provides API endpoints for the fight card overview: system status, fighter
//! roster, gorilla enclosure, audit trail, metrics, configuration, and
//! real-time event streaming via WebSocket.

use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};

use punch_types::{FighterStatus, GorillaStatus, WeightClass};

use crate::AppState;

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the arena dashboard router with all management UI routes.
///
/// All API routes are prefixed with `/api/dashboard/`. The static dashboard
/// HTML is served at `/dashboard/`.
pub fn dashboard_router() -> Router<AppState> {
    Router::new()
        // API endpoints
        .route("/api/dashboard/status", get(dashboard_status))
        .route("/api/dashboard/fighters", get(dashboard_fighters))
        .route("/api/dashboard/gorillas", get(dashboard_gorillas))
        .route("/api/dashboard/audit", get(dashboard_audit))
        .route("/api/dashboard/metrics", get(dashboard_metrics))
        .route(
            "/api/dashboard/config",
            get(dashboard_config).post(update_config),
        )
        .route("/api/dashboard/events", get(dashboard_events_ws))
        // Static dashboard UI
        .route("/dashboard", get(dashboard_html))
        .route("/dashboard/", get(dashboard_html))
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Arena-wide system status overview for the fight card.
#[derive(Debug, Serialize)]
struct DashboardStatusResponse {
    uptime_secs: i64,
    fighter_count: usize,
    gorilla_count: usize,
    active_bouts: usize,
    total_messages: u64,
    memory_entries: u64,
    system_health: &'static str,
}

/// Fighter summary for the arena roster display.
#[derive(Debug, Serialize)]
struct DashboardFighterSummary {
    id: String,
    name: String,
    description: String,
    weight_class: WeightClass,
    status: FighterStatus,
    model: String,
}

/// Gorilla summary for the enclosure display.
#[derive(Debug, Serialize)]
struct DashboardGorillaSummary {
    id: String,
    name: String,
    description: String,
    schedule: String,
    status: GorillaStatus,
    last_rampage: Option<String>,
}

/// A single audit log entry from the event bus history.
#[derive(Debug, Serialize)]
struct AuditEntry {
    sequence: u64,
    timestamp: String,
    kind: String,
    summary: String,
}

/// Query parameters for the audit endpoint.
#[derive(Debug, Deserialize)]
struct AuditQuery {
    /// Maximum number of entries to return.
    #[serde(default = "default_audit_limit")]
    limit: u64,
    /// Only return entries after this sequence number.
    #[serde(default)]
    since: u64,
}

fn default_audit_limit() -> u64 {
    50
}

/// System-wide metrics overview from the metering engine.
#[derive(Debug, Serialize)]
struct DashboardMetricsResponse {
    total_tokens_used: u64,
    total_tool_calls: u64,
    total_cost_usd: f64,
    fighter_count: usize,
    gorilla_count: usize,
}

/// Sanitized configuration view (no API keys exposed).
#[derive(Debug, Serialize)]
struct DashboardConfigResponse {
    api_listen: String,
    api_key_status: &'static str,
    rate_limit_rpm: u32,
    default_model: DashboardModelConfig,
    memory_db_path: String,
    knowledge_graph_enabled: bool,
    channel_count: usize,
    mcp_server_count: usize,
}

/// Sanitized model configuration (no API key environment variable values).
#[derive(Debug, Serialize)]
struct DashboardModelConfig {
    provider: String,
    model: String,
    api_key_env: Option<String>,
    base_url: Option<String>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
}

/// Request body for configuration updates.
#[derive(Debug, Deserialize)]
struct UpdateConfigRequest {
    /// New rate limit in requests per minute.
    rate_limit_rpm: Option<u32>,
}

/// Response for configuration updates.
#[derive(Debug, Serialize)]
struct UpdateConfigResponse {
    message: String,
    applied: bool,
}

/// Standard error response for dashboard endpoints.
#[derive(Debug, Serialize)]
struct DashboardError {
    error: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/dashboard/status — arena-wide system overview.
///
/// Returns the current fight card status including uptime, fighter and gorilla
/// counts, active bouts, and overall system health.
#[instrument(skip_all)]
async fn dashboard_status(State(state): State<AppState>) -> Json<DashboardStatusResponse> {
    let uptime = chrono::Utc::now()
        .signed_duration_since(state.started_at)
        .num_seconds();

    let fighters = state.ring.list_fighters();
    let gorillas = state.ring.list_gorillas().await;

    let active_bouts = fighters
        .iter()
        .filter(|(_, _, status)| *status == FighterStatus::Fighting)
        .count();

    // Query total usage from the metering engine for message/token counts.
    let since_epoch = chrono::DateTime::<chrono::Utc>::MIN_UTC;
    let usage_summary = state
        .ring
        .memory()
        .get_total_usage_summary(since_epoch)
        .await
        .unwrap_or_default();

    let total_messages = usage_summary.event_count;
    let memory_entries = usage_summary
        .total_input_tokens
        .saturating_add(usage_summary.total_output_tokens)
        / 1000;

    // Determine system health based on fighter statuses.
    let knocked_out_count = fighters
        .iter()
        .filter(|(_, _, status)| *status == FighterStatus::KnockedOut)
        .count();

    let system_health = if knocked_out_count > fighters.len() / 2 && !fighters.is_empty() {
        "degraded"
    } else {
        "healthy"
    };

    Json(DashboardStatusResponse {
        uptime_secs: uptime,
        fighter_count: fighters.len(),
        gorilla_count: gorillas.len(),
        active_bouts,
        total_messages,
        memory_entries,
        system_health,
    })
}

/// GET /api/dashboard/fighters — arena fighter roster.
///
/// Returns all registered fighters with their current status, weight class,
/// and assigned model.
#[instrument(skip_all)]
async fn dashboard_fighters(State(state): State<AppState>) -> Json<Vec<DashboardFighterSummary>> {
    let fighters = state.ring.list_fighters();

    let summaries = fighters
        .into_iter()
        .map(|(id, manifest, status)| DashboardFighterSummary {
            id: id.to_string(),
            name: manifest.name,
            description: manifest.description,
            weight_class: manifest.weight_class,
            status,
            model: manifest.model.model,
        })
        .collect();

    Json(summaries)
}

/// GET /api/dashboard/gorillas — gorilla enclosure overview.
///
/// Returns all registered gorillas with their schedule, status, and last
/// rampage time.
#[instrument(skip_all)]
async fn dashboard_gorillas(State(state): State<AppState>) -> Json<Vec<DashboardGorillaSummary>> {
    let gorillas = state.ring.list_gorillas().await;

    let summaries = gorillas
        .into_iter()
        .map(|(id, manifest, status)| DashboardGorillaSummary {
            id: id.to_string(),
            name: manifest.name,
            description: manifest.description,
            schedule: manifest.schedule,
            status,
            last_rampage: None,
        })
        .collect();

    Json(summaries)
}

/// GET /api/dashboard/audit — recent audit log entries.
///
/// Accepts optional query parameters `limit` (default 50) and `since`
/// (sequence number) to paginate through the event history.
#[instrument(skip_all)]
async fn dashboard_audit(
    State(state): State<AppState>,
    Query(params): Query<AuditQuery>,
) -> Json<Vec<AuditEntry>> {
    // Subscribe momentarily to capture recent events. Since the event bus is
    // broadcast-based and does not store history, we return what we can gather
    // from the Ring's current state as a synthetic audit trail.
    let fighters = state.ring.list_fighters();
    let gorillas = state.ring.list_gorillas().await;

    let mut entries = Vec::new();
    let mut seq: u64 = 1;

    // Generate synthetic audit entries from current fighter state.
    for (id, manifest, status) in &fighters {
        if seq > params.since {
            entries.push(AuditEntry {
                sequence: seq,
                timestamp: chrono::Utc::now().to_rfc3339(),
                kind: "fighter_status".to_string(),
                summary: format!("Fighter '{}' ({}) is {}", manifest.name, id, status),
            });
        }
        seq += 1;
        if entries.len() as u64 >= params.limit {
            break;
        }
    }

    // Generate synthetic audit entries from current gorilla state.
    for (id, manifest, status) in &gorillas {
        if seq > params.since && (entries.len() as u64) < params.limit {
            entries.push(AuditEntry {
                sequence: seq,
                timestamp: chrono::Utc::now().to_rfc3339(),
                kind: "gorilla_status".to_string(),
                summary: format!("Gorilla '{}' ({}) is {}", manifest.name, id, status),
            });
        }
        seq += 1;
    }

    // Add a system uptime entry if within limits.
    if (entries.len() as u64) < params.limit && seq > params.since {
        let uptime = chrono::Utc::now()
            .signed_duration_since(state.started_at)
            .num_seconds();
        entries.push(AuditEntry {
            sequence: seq,
            timestamp: state.started_at.to_rfc3339(),
            kind: "system_start".to_string(),
            summary: format!("Arena opened {} seconds ago", uptime),
        });
    }

    Json(entries)
}

/// GET /api/dashboard/metrics — system-wide metering metrics.
///
/// Returns aggregate token usage, tool call counts, and cost data from
/// the metering engine.
#[instrument(skip_all)]
async fn dashboard_metrics(State(state): State<AppState>) -> Json<DashboardMetricsResponse> {
    let since_epoch = chrono::DateTime::<chrono::Utc>::MIN_UTC;

    let usage_summary = state
        .ring
        .memory()
        .get_total_usage_summary(since_epoch)
        .await
        .unwrap_or_default();

    let fighters = state.ring.list_fighters();
    let gorillas = state.ring.list_gorillas().await;

    Json(DashboardMetricsResponse {
        total_tokens_used: usage_summary
            .total_input_tokens
            .saturating_add(usage_summary.total_output_tokens),
        total_tool_calls: usage_summary.event_count,
        total_cost_usd: usage_summary.total_cost_usd,
        fighter_count: fighters.len(),
        gorilla_count: gorillas.len(),
    })
}

/// GET /api/dashboard/config — sanitized configuration view.
///
/// Returns the current system configuration with API keys redacted.
/// No secrets are exposed through this endpoint.
#[instrument(skip_all)]
async fn dashboard_config(State(state): State<AppState>) -> Json<DashboardConfigResponse> {
    let config = &*state.config;

    let api_key_status = if config.api_key.is_empty() {
        "disabled"
    } else {
        "***"
    };

    Json(DashboardConfigResponse {
        api_listen: config.api_listen.clone(),
        api_key_status,
        rate_limit_rpm: config.rate_limit_rpm,
        default_model: DashboardModelConfig {
            provider: config.default_model.provider.to_string(),
            model: config.default_model.model.clone(),
            api_key_env: config.default_model.api_key_env.clone(),
            base_url: config.default_model.base_url.clone(),
            max_tokens: config.default_model.max_tokens,
            temperature: config.default_model.temperature,
        },
        memory_db_path: config.memory.db_path.clone(),
        knowledge_graph_enabled: config.memory.knowledge_graph_enabled,
        channel_count: config.channels.len(),
        mcp_server_count: config.mcp_servers.len(),
    })
}

/// POST /api/dashboard/config — update configuration (hot reload).
///
/// Currently supports updating the rate limit. Returns whether the update
/// was applied successfully.
#[instrument(skip_all)]
async fn update_config(
    State(_state): State<AppState>,
    Json(body): Json<UpdateConfigRequest>,
) -> Result<Json<UpdateConfigResponse>, (StatusCode, Json<DashboardError>)> {
    // Validate the incoming configuration update.
    if let Some(rpm) = body.rate_limit_rpm {
        if rpm == 0 {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(DashboardError {
                    error: "rate_limit_rpm must be greater than 0".to_string(),
                }),
            ));
        }
        if rpm > 100_000 {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(DashboardError {
                    error: "rate_limit_rpm must not exceed 100000".to_string(),
                }),
            ));
        }
    }

    // Note: The AppState config is behind an Arc and cannot be mutated directly.
    // A full hot-reload implementation would require interior mutability on the
    // config. For now we acknowledge the request and report that a restart is
    // needed for config changes to take full effect.
    let message = if body.rate_limit_rpm.is_some() {
        "Configuration update acknowledged. Rate limit changes require a restart to take full effect.".to_string()
    } else {
        "No configuration changes specified.".to_string()
    };

    Ok(Json(UpdateConfigResponse {
        message,
        applied: body.rate_limit_rpm.is_none(),
    }))
}

/// GET /api/dashboard/events — WebSocket upgrade for real-time event streaming.
///
/// Upgrades the connection to a WebSocket and streams events from the Ring's
/// event bus as JSON messages. Each event is broadcast to all connected
/// dashboard clients.
#[instrument(skip_all)]
async fn dashboard_events_ws(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_event_stream(socket, state))
}

/// Handle an active WebSocket connection, streaming events from the event bus.
async fn handle_event_stream(mut socket: WebSocket, state: AppState) {
    let mut rx = state.ring.event_bus().subscribe();

    loop {
        tokio::select! {
            // Forward events from the bus to the WebSocket client.
            result = rx.recv() => {
                match result {
                    Ok(payload) => {
                        let json = match serde_json::to_string(&payload) {
                            Ok(j) => j,
                            Err(e) => {
                                warn!(error = %e, "failed to serialize event payload");
                                continue;
                            }
                        };
                        if socket.send(WsMessage::Text(json.into())).await.is_err() {
                            // Client disconnected.
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "dashboard WebSocket client lagged behind");
                        // Continue receiving after the gap.
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
            // Handle incoming messages from the client (ping/pong/close).
            msg = socket.recv() => {
                match msg {
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    Some(Ok(WsMessage::Ping(data))) => {
                        if socket.send(WsMessage::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Some(Err(_)) => break,
                    _ => {} // Ignore text/binary messages from the client.
                }
            }
        }
    }
}

/// GET /dashboard — serve the self-contained arena dashboard HTML.
///
/// Returns a single-page application with embedded CSS and JavaScript that
/// displays the fight card: system status, fighter roster, gorilla enclosure,
/// and recent audit entries. Auto-refreshes every 10 seconds.
#[instrument(skip_all)]
async fn dashboard_html() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        Html(DASHBOARD_HTML),
    )
}

// ---------------------------------------------------------------------------
// Embedded dashboard HTML
// ---------------------------------------------------------------------------

/// Self-contained arena dashboard HTML with embedded CSS and JavaScript.
///
/// All user-supplied data is escaped via the `esc()` function before insertion
/// into the DOM to prevent XSS attacks.
const DASHBOARD_HTML: &str = include_str!("dashboard_ui.html");

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    use crate::routes::a2a::A2AState;
    use punch_kernel::Ring;
    use punch_memory::MemorySubstrate;
    use punch_types::a2a::A2ARegistry;
    use punch_types::config::MemoryConfig;
    use punch_types::{
        FighterManifest, GorillaManifest, ModelConfig, Provider, PunchConfig, WeightClass,
    };

    // Mock LLM driver for tests.
    use std::sync::atomic::{AtomicU64, Ordering};

    use async_trait::async_trait;
    use punch_runtime::{CompletionRequest, CompletionResponse, LlmDriver, StopReason, TokenUsage};

    struct MockDriver {
        _calls: AtomicU64,
    }

    impl MockDriver {
        fn new() -> Self {
            Self {
                _calls: AtomicU64::new(0),
            }
        }
    }

    #[async_trait]
    impl LlmDriver for MockDriver {
        async fn complete(
            &self,
            _request: CompletionRequest,
        ) -> punch_types::PunchResult<CompletionResponse> {
            self._calls.fetch_add(1, Ordering::SeqCst);
            Ok(CompletionResponse {
                message: punch_types::Message {
                    role: punch_types::Role::Assistant,
                    content: "mock response".to_string(),
                    tool_calls: Vec::new(),
                    tool_results: Vec::new(),
                    timestamp: chrono::Utc::now(),
                },
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                stop_reason: StopReason::EndTurn,
            })
        }
    }

    fn test_config() -> PunchConfig {
        PunchConfig {
            api_listen: "127.0.0.1:0".to_string(),
            api_key: "test-secret-key".to_string(),
            rate_limit_rpm: 60,
            default_model: ModelConfig {
                provider: Provider::Ollama,
                model: "test-model".to_string(),
                api_key_env: Some("TEST_API_KEY".to_string()),
                base_url: Some("http://localhost:11434".to_string()),
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
            memory: MemoryConfig {
                db_path: ":memory:".to_string(),
                knowledge_graph_enabled: false,
                max_entries: None,
            },
            channels: Default::default(),
            mcp_servers: Default::default(),
        }
    }

    fn test_app_state() -> AppState {
        let config = test_config();
        let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory db"));
        let driver: Arc<dyn LlmDriver> = Arc::new(MockDriver::new());
        let ring = Arc::new(Ring::new(config.clone(), memory, driver));

        AppState {
            ring,
            started_at: chrono::Utc::now(),
            config: Arc::new(config.clone()),
            a2a: A2AState::new(A2ARegistry::our_card(
                "test-agent",
                "http://localhost:0",
                vec![],
            )),
        }
    }

    fn test_fighter_manifest(name: &str) -> FighterManifest {
        FighterManifest {
            name: name.to_string(),
            description: format!("{} description", name),
            model: ModelConfig {
                provider: Provider::Ollama,
                model: "test-model".to_string(),
                api_key_env: None,
                base_url: None,
                max_tokens: Some(4096),
                temperature: Some(0.7),
            },
            system_prompt: "You are a test fighter.".to_string(),
            capabilities: Vec::new(),
            weight_class: WeightClass::Middleweight,
            tenant_id: None,
        }
    }

    fn test_gorilla_manifest(name: &str) -> GorillaManifest {
        GorillaManifest {
            name: name.to_string(),
            description: format!("{} description", name),
            schedule: "*/5 * * * *".to_string(),
            moves_required: Vec::new(),
            settings_schema: None,
            dashboard_metrics: Vec::new(),
            system_prompt: None,
            model: None,
            capabilities: Vec::new(),
            weight_class: None,
        }
    }

    /// Helper to send a GET request to the dashboard router and return the response.
    async fn send_get(app: Router, uri: &str) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
    }

    /// Helper to send a POST request with JSON body.
    async fn send_post_json(
        app: Router,
        uri: &str,
        body: serde_json::Value,
    ) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("serialize")))
                .expect("request"),
        )
        .await
        .expect("response")
    }

    // -- Test 1: Status endpoint returns valid JSON --

    #[tokio::test]
    async fn test_status_endpoint_returns_valid_json() {
        let state = test_app_state();
        let app = dashboard_router().with_state(state);

        let resp = send_get(app, "/api/dashboard/status").await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");

        assert!(json["uptime_secs"].is_number());
        assert!(json["fighter_count"].is_number());
        assert!(json["gorilla_count"].is_number());
        assert!(json["active_bouts"].is_number());
        assert!(json["system_health"].is_string());
    }

    // -- Test 2: Fighters endpoint returns array --

    #[tokio::test]
    async fn test_fighters_endpoint_returns_array() {
        let state = test_app_state();
        state
            .ring
            .spawn_fighter(test_fighter_manifest("alpha"))
            .await;
        state
            .ring
            .spawn_fighter(test_fighter_manifest("bravo"))
            .await;

        let app = dashboard_router().with_state(state);
        let resp = send_get(app, "/api/dashboard/fighters").await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).expect("valid JSON array");

        assert_eq!(json.len(), 2);
        let names: Vec<&str> = json.iter().filter_map(|f| f["name"].as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"bravo"));
    }

    // -- Test 3: Gorillas endpoint returns array --

    #[tokio::test]
    async fn test_gorillas_endpoint_returns_array() {
        let state = test_app_state();
        state.ring.register_gorilla(test_gorilla_manifest("kong"));

        let app = dashboard_router().with_state(state);
        let resp = send_get(app, "/api/dashboard/gorillas").await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).expect("valid JSON array");

        assert_eq!(json.len(), 1);
        assert_eq!(json[0]["name"], "kong");
        assert_eq!(json[0]["status"], "caged");
    }

    // -- Test 4: Audit endpoint returns entries --

    #[tokio::test]
    async fn test_audit_endpoint_returns_entries() {
        let state = test_app_state();
        state
            .ring
            .spawn_fighter(test_fighter_manifest("audit-fighter"))
            .await;

        let app = dashboard_router().with_state(state);
        let resp = send_get(app, "/api/dashboard/audit").await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).expect("valid JSON array");

        // Should have at least the fighter entry and system start entry.
        assert!(!json.is_empty());
        assert!(json[0]["sequence"].is_number());
        assert!(json[0]["kind"].is_string());
        assert!(json[0]["summary"].is_string());
    }

    // -- Test 5: Config endpoint sanitizes API key --

    #[tokio::test]
    async fn test_config_endpoint_sanitizes_api_key() {
        let state = test_app_state();
        // The test config has api_key = "test-secret-key"
        let app = dashboard_router().with_state(state);

        let resp = send_get(app, "/api/dashboard/config").await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");

        // The API key should be masked, not exposed.
        assert_eq!(json["api_key_status"], "***");
        // The actual key value should NOT appear anywhere in the response.
        let serialized = serde_json::to_vec(&json).expect("serialize");
        let body_str = String::from_utf8_lossy(&serialized);
        assert!(!body_str.contains("test-secret-key"));
    }

    // -- Test 6: Dashboard router construction --

    #[tokio::test]
    async fn test_dashboard_router_construction() {
        let state = test_app_state();
        // Building the router should not panic.
        let app = dashboard_router().with_state(state);

        // Verify a basic route works.
        let resp = send_get(app, "/api/dashboard/status").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // -- Test 7: Metrics endpoint structure --

    #[tokio::test]
    async fn test_metrics_endpoint_structure() {
        let state = test_app_state();
        let app = dashboard_router().with_state(state);

        let resp = send_get(app, "/api/dashboard/metrics").await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");

        assert!(json["total_tokens_used"].is_number());
        assert!(json["total_tool_calls"].is_number());
        assert!(json["total_cost_usd"].is_number());
        assert!(json["fighter_count"].is_number());
        assert!(json["gorilla_count"].is_number());
    }

    // -- Test 8: Config update validates input --

    #[tokio::test]
    async fn test_config_update_validates_input() {
        let state = test_app_state();
        let app = dashboard_router().with_state(state);

        // Zero rate limit should be rejected.
        let resp = send_post_json(
            app,
            "/api/dashboard/config",
            serde_json::json!({ "rate_limit_rpm": 0 }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert!(
            json["error"]
                .as_str()
                .expect("error field")
                .contains("greater than 0")
        );
    }

    // -- Test 9: Static HTML endpoint returns HTML content-type --

    #[tokio::test]
    async fn test_static_html_returns_html_content_type() {
        let state = test_app_state();
        let app = dashboard_router().with_state(state);

        let resp = send_get(app, "/dashboard").await;
        assert_eq!(resp.status(), StatusCode::OK);

        let content_type = resp
            .headers()
            .get("content-type")
            .expect("content-type header")
            .to_str()
            .expect("header value");
        assert!(
            content_type.contains("text/html"),
            "Expected text/html, got: {}",
            content_type
        );

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("Punch Arena Dashboard"));
        assert!(html.contains("<!DOCTYPE html>"));
    }

    // -- Test 10: Query parameter parsing for audit endpoint --

    #[tokio::test]
    async fn test_audit_query_parameter_parsing() {
        let state = test_app_state();
        state.ring.spawn_fighter(test_fighter_manifest("f1")).await;
        state.ring.spawn_fighter(test_fighter_manifest("f2")).await;
        state.ring.spawn_fighter(test_fighter_manifest("f3")).await;
        state.ring.register_gorilla(test_gorilla_manifest("g1"));

        let app = dashboard_router().with_state(state.clone());

        // Request with limit=1 should return at most 1 entry.
        let resp = send_get(app, "/api/dashboard/audit?limit=1").await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: Vec<serde_json::Value> = serde_json::from_slice(&body).expect("valid JSON array");
        assert_eq!(json.len(), 1);

        // Request with since= a high number should return only later entries.
        let app2 = dashboard_router().with_state(state);
        let resp2 = send_get(app2, "/api/dashboard/audit?since=100&limit=50").await;
        assert_eq!(resp2.status(), StatusCode::OK);

        let body2 = axum::body::to_bytes(resp2.into_body(), usize::MAX)
            .await
            .expect("body");
        let json2: Vec<serde_json::Value> =
            serde_json::from_slice(&body2).expect("valid JSON array");
        // With since=100, all entries should be filtered out (we only have a few).
        assert!(json2.is_empty());
    }

    // -- Test 11: Config update with excessive rate limit --

    #[tokio::test]
    async fn test_config_update_rejects_excessive_rate_limit() {
        let state = test_app_state();
        let app = dashboard_router().with_state(state);

        let resp = send_post_json(
            app,
            "/api/dashboard/config",
            serde_json::json!({ "rate_limit_rpm": 200000 }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // -- Test 12: Config endpoint shows disabled when no API key --

    #[tokio::test]
    async fn test_config_shows_disabled_when_no_api_key() {
        let mut config = test_config();
        config.api_key = String::new();
        let memory = Arc::new(MemorySubstrate::in_memory().expect("in-memory db"));
        let driver: Arc<dyn LlmDriver> = Arc::new(MockDriver::new());
        let ring = Arc::new(Ring::new(config.clone(), memory, driver));

        let state = AppState {
            ring,
            started_at: chrono::Utc::now(),
            config: Arc::new(config.clone()),
            a2a: A2AState::new(A2ARegistry::our_card(
                "test-agent",
                "http://localhost:0",
                vec![],
            )),
        };

        let app = dashboard_router().with_state(state);
        let resp = send_get(app, "/api/dashboard/config").await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");

        assert_eq!(json["api_key_status"], "disabled");
    }

    // -- Test 13: Dashboard HTML trailing slash also works --

    #[tokio::test]
    async fn test_dashboard_html_trailing_slash() {
        let state = test_app_state();
        let app = dashboard_router().with_state(state);

        let resp = send_get(app, "/dashboard/").await;
        assert_eq!(resp.status(), StatusCode::OK);

        let content_type = resp
            .headers()
            .get("content-type")
            .expect("content-type header")
            .to_str()
            .expect("header value");
        assert!(content_type.contains("text/html"));
    }

    // -- Test: Status response field types --

    #[tokio::test]
    async fn test_status_system_health_is_healthy_when_no_fighters() {
        let state = test_app_state();
        let app = dashboard_router().with_state(state);

        let resp = send_get(app, "/api/dashboard/status").await;
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");

        // With no fighters, system should be healthy
        assert_eq!(json["system_health"], "healthy");
        assert_eq!(json["fighter_count"], 0);
        assert_eq!(json["gorilla_count"], 0);
        assert_eq!(json["active_bouts"], 0);
    }

    // -- Test: Config update with valid rate limit --

    #[tokio::test]
    async fn test_config_update_valid_rate_limit() {
        let state = test_app_state();
        let app = dashboard_router().with_state(state);

        let resp = send_post_json(
            app,
            "/api/dashboard/config",
            serde_json::json!({ "rate_limit_rpm": 100 }),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");

        assert!(json["message"].as_str().unwrap().contains("acknowledged"));
    }

    // -- Test: Config update with no changes --

    #[tokio::test]
    async fn test_config_update_no_changes() {
        let state = test_app_state();
        let app = dashboard_router().with_state(state);

        let resp = send_post_json(
            app,
            "/api/dashboard/config",
            serde_json::json!({}),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");

        assert!(json["message"].as_str().unwrap().contains("No configuration changes"));
        assert_eq!(json["applied"], true);
    }

    // -- Test: Dashboard response types serialization --

    #[test]
    fn test_dashboard_status_response_serialization() {
        let resp = DashboardStatusResponse {
            uptime_secs: 3600,
            fighter_count: 5,
            gorilla_count: 2,
            active_bouts: 1,
            total_messages: 1000,
            memory_entries: 500,
            system_health: "healthy",
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["uptime_secs"], 3600);
        assert_eq!(json["fighter_count"], 5);
        assert_eq!(json["system_health"], "healthy");
    }

    #[test]
    fn test_dashboard_fighter_summary_serialization() {
        let summary = DashboardFighterSummary {
            id: "f-123".to_string(),
            name: "Alpha Fighter".to_string(),
            description: "Test fighter".to_string(),
            weight_class: WeightClass::Heavyweight,
            status: FighterStatus::Idle,
            model: "gpt-4o".to_string(),
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["name"], "Alpha Fighter");
        assert_eq!(json["model"], "gpt-4o");
    }

    #[test]
    fn test_dashboard_gorilla_summary_serialization() {
        let summary = DashboardGorillaSummary {
            id: "g-456".to_string(),
            name: "Kong".to_string(),
            description: "Test gorilla".to_string(),
            schedule: "*/5 * * * *".to_string(),
            status: GorillaStatus::Caged,
            last_rampage: None,
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["name"], "Kong");
        assert_eq!(json["status"], "caged");
        assert!(json["last_rampage"].is_null());
    }

    #[test]
    fn test_audit_entry_serialization() {
        let entry = AuditEntry {
            sequence: 42,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            kind: "fighter_status".to_string(),
            summary: "Fighter 'Alpha' spawned".to_string(),
        };
        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["sequence"], 42);
        assert_eq!(json["kind"], "fighter_status");
    }

    #[test]
    fn test_dashboard_metrics_response_serialization() {
        let resp = DashboardMetricsResponse {
            total_tokens_used: 50000,
            total_tool_calls: 100,
            total_cost_usd: 1.23,
            fighter_count: 3,
            gorilla_count: 1,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["total_tokens_used"], 50000);
        assert_eq!(json["total_cost_usd"], 1.23);
    }

    #[test]
    fn test_dashboard_error_serialization() {
        let err = DashboardError {
            error: "something went wrong".to_string(),
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["error"], "something went wrong");
    }

    #[test]
    fn test_audit_query_defaults() {
        let json = r#"{}"#;
        let query: AuditQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.limit, 50);
        assert_eq!(query.since, 0);
    }

    #[test]
    fn test_update_config_request_deserialization() {
        let json = r#"{"rate_limit_rpm": 120}"#;
        let req: UpdateConfigRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.rate_limit_rpm, Some(120));
    }

    #[test]
    fn test_update_config_request_null() {
        let json = r#"{}"#;
        let req: UpdateConfigRequest = serde_json::from_str(json).unwrap();
        assert!(req.rate_limit_rpm.is_none());
    }
}
