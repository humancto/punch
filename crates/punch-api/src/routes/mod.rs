//! Route module organisation for The Arena API.

pub mod channels;
pub mod chat;
pub mod fighters;
pub mod gorillas;
pub mod health;
pub mod triggers;
pub mod workflows;

use axum::Router;

use crate::AppState;

/// Build the combined API router with all route groups.
pub fn api_router() -> Router<AppState> {
    Router::new()
        .merge(health::router())
        .merge(fighters::router())
        .merge(gorillas::router())
        .merge(chat::router())
        .merge(workflows::router())
        .merge(channels::router())
        .merge(triggers::router())
}
