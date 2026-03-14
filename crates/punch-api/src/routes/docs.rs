//! OpenAPI schema and Swagger UI documentation endpoints.

use axum::http::header;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::{Json, Router};

use crate::openapi;
use crate::AppState;

/// Build the documentation routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/openapi.json", get(openapi_json))
        .route("/api/docs", get(swagger_ui))
}

/// GET /api/openapi.json — returns the OpenAPI 3.0.3 schema as JSON.
async fn openapi_json() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        Json(openapi::openapi_schema()),
    )
}

/// GET /api/docs — returns a Swagger UI HTML page.
async fn swagger_ui() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        Html(openapi::swagger_ui_html()),
    )
}
