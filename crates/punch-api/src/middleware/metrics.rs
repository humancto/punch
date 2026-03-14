//! Request metrics middleware.
//!
//! Automatically records per-request metrics (count, duration, active requests)
//! for every HTTP request flowing through The Arena.

use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, Response};
use axum::middleware::Next;

use punch_kernel::metrics::{self, MetricsRegistry};

/// Shared metrics state threaded through the middleware layer.
#[derive(Clone)]
pub struct MetricsMiddlewareState {
    pub registry: Arc<MetricsRegistry>,
}

/// Metrics middleware that records request count, duration, and active request gauge.
///
/// Skips recording for `/metrics` itself to avoid self-referential noise.
pub async fn metrics_middleware(
    State(state): State<MetricsMiddlewareState>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let path = request.uri().path().to_string();

    // Skip recording for the metrics endpoint itself.
    if path == "/metrics" {
        return next.run(request).await;
    }

    let method = request.method().to_string();

    // Track active requests.
    state
        .registry
        .gauge_inc("punch_active_requests");

    let start = Instant::now();
    let response = next.run(request).await;
    let duration = start.elapsed().as_secs_f64();

    // Decrement active requests.
    state
        .registry
        .gauge_dec("punch_active_requests");

    let status = response.status().as_u16().to_string();

    // Record request count.
    state.registry.counter_with_labels(
        metrics::REQUESTS_TOTAL,
        &[("method", &method), ("path", &path), ("status", &status)],
    );

    // Record request duration.
    state
        .registry
        .histogram_observe(metrics::REQUEST_DURATION_SECONDS, duration);

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_middleware_state_clone() {
        let state = MetricsMiddlewareState {
            registry: Arc::new(MetricsRegistry::new()),
        };
        let _cloned = state.clone();
    }
}
