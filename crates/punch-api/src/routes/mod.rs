//! Route module organisation for The Arena API.

pub mod a2a;
pub mod budget;
pub mod channels;
pub mod chat;
pub mod dashboard;
pub mod docs;
pub mod fighters;
pub mod gorillas;
pub mod health;
pub mod metrics;
pub mod openai_compat;
pub mod tenants;
pub mod triggers;
pub mod troops;
pub mod workflows;

use axum::Router;

use crate::AppState;

/// Build the combined API router with all route groups.
pub fn api_router() -> Router<AppState> {
    Router::new()
        .merge(health::router())
        .merge(fighters::router())
        .merge(gorillas::router())
        .merge(openai_compat::router())
        .merge(workflows::router())
        .merge(channels::router())
        .merge(triggers::router())
        .merge(troops::router())
        .merge(dashboard::dashboard_router())
        .merge(metrics::router())
        .merge(a2a::router())
        .merge(budget::router())
        .merge(tenants::router())
        .merge(docs::router())
}
