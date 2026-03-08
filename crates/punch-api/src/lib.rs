//! # punch-api
//!
//! **The Arena** — the HTTP API server where external clients interact with The Ring.
//!
//! This crate provides an Axum-based REST API for spawning fighters, managing
//! gorillas, sending messages, and exposing an OpenAI-compatible chat endpoint.

pub mod routes;
pub mod server;

use std::sync::Arc;

use punch_kernel::Ring;

/// Shared application state threaded through all Axum handlers.
#[derive(Clone)]
pub struct AppState {
    /// The Ring — central kernel and coordinator.
    pub ring: Arc<Ring>,
    /// Server start time for uptime tracking.
    pub started_at: chrono::DateTime<chrono::Utc>,
}
