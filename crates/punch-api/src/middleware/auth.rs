//! Bearer token / API key authentication middleware.
//!
//! When an API key is configured (via `PunchConfig::api_key` or `PUNCH_API_KEY`
//! environment variable), all requests to `/api/*` and `/v1/*` must include a
//! valid credential. The `/health` endpoint is always public.
//!
//! Supported credential formats:
//!   - `Authorization: Bearer <token>`
//!   - `X-API-Key: <key>`
//!
//! If no API key is configured, all requests are allowed (dev mode).

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::middleware::Next;

/// Authentication middleware.
///
/// `api_key` is the expected key. If empty, auth is bypassed entirely.
pub async fn auth_middleware(
    axum::extract::State(api_key): axum::extract::State<String>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    // If no API key configured, skip authentication (dev mode).
    if api_key.is_empty() {
        return next.run(request).await;
    }

    // Always allow /health without auth.
    let path = request.uri().path();
    if path == "/health" {
        return next.run(request).await;
    }

    // Check Authorization: Bearer <token>
    let bearer_token = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    // Fallback to X-API-Key header
    let token = bearer_token.or_else(|| {
        request
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
    });

    match token {
        Some(t) if constant_time_eq(t, &api_key) => next.run(request).await,
        Some(_) => {
            // Invalid key provided
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("content-type", "application/json")
                .header("www-authenticate", "Bearer")
                .body(Body::from(
                    r#"{"error":{"message":"Invalid API key","type":"authentication_error","code":"invalid_api_key"}}"#,
                ))
                .unwrap_or_default()
        }
        None => {
            // No key provided
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("content-type", "application/json")
                .header("www-authenticate", "Bearer")
                .body(Body::from(
                    r#"{"error":{"message":"Missing API key. Provide via Authorization: Bearer <key> or X-API-Key header","type":"authentication_error","code":"missing_api_key"}}"#,
                ))
                .unwrap_or_default()
        }
    }
}

/// Constant-time string comparison to prevent timing attacks.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes()
        .iter()
        .zip(b.as_bytes().iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_time_eq_match() {
        assert!(constant_time_eq("secret-key-123", "secret-key-123"));
    }

    #[test]
    fn test_constant_time_eq_mismatch() {
        assert!(!constant_time_eq("secret-key-123", "wrong-key-456"));
    }

    #[test]
    fn test_constant_time_eq_different_lengths() {
        assert!(!constant_time_eq("short", "much-longer-key"));
    }

    #[test]
    fn test_constant_time_eq_empty() {
        assert!(constant_time_eq("", ""));
    }
}
