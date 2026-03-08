//! Arena server setup and lifecycle.

use std::sync::Arc;

use axum::Router;
use axum::middleware;
use axum::response::Response;
use tokio::net::TcpListener;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

use punch_kernel::Ring;
use punch_types::{PunchConfig, PunchResult};

use crate::AppState;
use crate::routes;

/// Start The Arena — the HTTP API server.
///
/// Binds to the address specified in `config.api_listen` and serves until the
/// process is terminated.
pub async fn start_arena(ring: Arc<Ring>, config: &PunchConfig) -> PunchResult<()> {
    let state = AppState {
        ring,
        started_at: chrono::Utc::now(),
    };

    let app = build_router(state);

    let listener = TcpListener::bind(&config.api_listen).await?;
    info!(address = %config.api_listen, "the arena is open");

    axum::serve(listener, app).await?;

    Ok(())
}

/// Build the Axum router with all routes and middleware.
fn build_router(state: AppState) -> Router {
    let api = routes::api_router();

    Router::new()
        .merge(api)
        .layer(middleware::from_fn(security_headers))
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Middleware that adds security headers to every response.
async fn security_headers(
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        "nosniff".parse().unwrap(),
    );
    headers.insert(axum::http::header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        "no-store".parse().unwrap(),
    );

    response
}
