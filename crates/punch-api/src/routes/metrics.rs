//! Prometheus-compatible metrics export endpoint.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use tracing::instrument;

use crate::AppState;

/// Build the metrics routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/metrics", get(metrics_handler))
}

/// GET /metrics -- export metrics in Prometheus text exposition format.
#[instrument(skip_all)]
async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    let body = state.ring.metrics().export_prometheus();
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}
