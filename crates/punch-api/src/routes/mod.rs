//! Route module organisation for The Arena API.

pub mod chat;
pub mod fighters;
pub mod gorillas;
pub mod health;

use axum::Router;

use crate::AppState;

/// Build the combined API router with all route groups.
pub fn api_router() -> Router<AppState> {
    Router::new()
        .merge(health::router())
        .merge(fighters::router())
        .merge(gorillas::router())
        .merge(chat::router())
}
