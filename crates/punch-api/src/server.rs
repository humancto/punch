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

use punch_kernel::{A2ATaskExecutor, Ring};
use punch_types::a2a::A2ARegistry;
use punch_types::{PunchConfig, PunchResult};

use crate::AppState;
use crate::middleware::auth::{auth_middleware, tenant_auth_middleware};
use crate::middleware::metrics::{MetricsMiddlewareState, metrics_middleware};
use crate::middleware::rate_limit::{RateLimiterState, rate_limit_middleware};
use crate::routes;
use crate::routes::a2a::A2AState;

/// Start The Arena -- the HTTP API server.
///
/// Binds to the address specified in `config.api_listen` and serves until the
/// process is terminated.
pub async fn start_arena(ring: Arc<Ring>, config: &PunchConfig) -> PunchResult<()> {
    // Resolve the API key: prefer config, fall back to environment variable.
    let api_key = if config.api_key.is_empty() {
        std::env::var("PUNCH_API_KEY").unwrap_or_default()
    } else {
        config.api_key.clone()
    };

    // Build the local agent card for A2A discovery.
    let local_card = A2ARegistry::our_card(
        "punch-arena",
        &format!("http://{}", config.api_listen),
        vec!["coordination".to_string(), "task_delegation".to_string()],
    );

    let a2a_state = A2AState::new(local_card);

    // Start the A2A task executor so pending tasks get picked up by fighters.
    let mut a2a_executor = A2ATaskExecutor::new(Arc::clone(&ring), Arc::clone(&a2a_state.tasks));
    a2a_executor.start();

    let state = AppState {
        ring,
        started_at: chrono::Utc::now(),
        config: Arc::new(config.clone()),
        a2a: a2a_state,
    };

    let app = build_router(state, &api_key, config.rate_limit_rpm);

    let listener = TcpListener::bind(&config.api_listen).await?;
    info!(address = %config.api_listen, "the arena is open");

    axum::serve(listener, app).await?;

    // Stop the executor on shutdown.
    a2a_executor.stop();

    Ok(())
}

/// Build the Axum router with all routes and middleware.
///
/// Middleware is applied in this order (outermost first):
///   1. CORS
///   2. Tracing
///   3. Compression
///   4. Security headers
///   5. Rate limiting (skips /health)
///   6. Authentication (skips /health)
///   7. Routes
pub fn build_router(state: AppState, api_key: &str, rate_limit_rpm: u32) -> Router {
    let api = routes::api_router();
    let rate_limiter = RateLimiterState::new(rate_limit_rpm);
    let metrics_state = MetricsMiddlewareState {
        registry: Arc::clone(state.ring.metrics()),
    };

    Router::new()
        .merge(api)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            tenant_auth_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            api_key.to_string(),
            auth_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            rate_limiter,
            rate_limit_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            metrics_state,
            metrics_middleware,
        ))
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
