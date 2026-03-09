//! Per-IP sliding window rate limiting middleware.
//!
//! Uses a simple in-memory sliding window counter per IP address.
//! Default: 60 requests/minute per IP, configurable via `PunchConfig::rate_limit_rpm`.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::middleware::Next;
use dashmap::DashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;

/// State for the rate limiter.
#[derive(Clone)]
pub struct RateLimiterState {
    /// Maximum requests per window per IP.
    pub max_requests: u32,
    /// Window duration in seconds.
    pub window_secs: u64,
    /// Per-IP request timestamps within the current window.
    entries: Arc<DashMap<IpAddr, Vec<Instant>>>,
}

impl RateLimiterState {
    /// Create a new rate limiter with the given requests-per-minute limit.
    pub fn new(requests_per_minute: u32) -> Self {
        Self {
            max_requests: requests_per_minute,
            window_secs: 60,
            entries: Arc::new(DashMap::new()),
        }
    }

    /// Check if the IP is allowed and record the request.
    /// Returns the number of remaining requests, or None if rate limited.
    fn check(&self, ip: IpAddr) -> Option<u32> {
        let now = Instant::now();
        let window = std::time::Duration::from_secs(self.window_secs);

        let mut entry = self.entries.entry(ip).or_default();
        let timestamps = entry.value_mut();

        // Remove expired entries (outside the sliding window).
        timestamps.retain(|t| now.duration_since(*t) < window);

        if timestamps.len() as u32 >= self.max_requests {
            return None;
        }

        timestamps.push(now);
        Some(self.max_requests - timestamps.len() as u32)
    }
}

/// Rate limiting middleware.
///
/// Skips rate limiting for `/health`.
/// Returns 429 Too Many Requests with Retry-After header when exceeded.
pub async fn rate_limit_middleware(
    axum::extract::State(limiter): axum::extract::State<RateLimiterState>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    // Skip rate limiting for /health.
    let path = request.uri().path();
    if path == "/health" {
        return next.run(request).await;
    }

    // Extract client IP from ConnectInfo or default to loopback.
    let ip = request
        .extensions()
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or(IpAddr::from([127, 0, 0, 1]));

    match limiter.check(ip) {
        Some(remaining) => {
            let mut response = next.run(request).await;
            // Add rate limit headers.
            let headers = response.headers_mut();
            if let Ok(v) = limiter.max_requests.to_string().parse() {
                headers.insert("x-ratelimit-limit", v);
            }
            if let Ok(v) = remaining.to_string().parse() {
                headers.insert("x-ratelimit-remaining", v);
            }
            response
        }
        None => {
            tracing::warn!(%ip, limit = limiter.max_requests, "rate limit exceeded");
            Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header("content-type", "application/json")
                .header("retry-after", limiter.window_secs.to_string())
                .header("x-ratelimit-limit", limiter.max_requests.to_string())
                .header("x-ratelimit-remaining", "0")
                .body(Body::from(
                    r#"{"error":{"message":"Rate limit exceeded. Please retry after the Retry-After period.","type":"rate_limit_error","code":"rate_limit_exceeded"}}"#,
                ))
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_rate_limiter_allows_within_limit() {
        let limiter = RateLimiterState::new(5);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));

        for _ in 0..5 {
            assert!(limiter.check(ip).is_some());
        }
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let limiter = RateLimiterState::new(3);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        for _ in 0..3 {
            assert!(limiter.check(ip).is_some());
        }
        // 4th request should be blocked
        assert!(limiter.check(ip).is_none());
    }

    #[test]
    fn test_rate_limiter_different_ips_independent() {
        let limiter = RateLimiterState::new(2);
        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        // Exhaust ip1's quota
        assert!(limiter.check(ip1).is_some());
        assert!(limiter.check(ip1).is_some());
        assert!(limiter.check(ip1).is_none());

        // ip2 should still have its full quota
        assert!(limiter.check(ip2).is_some());
        assert!(limiter.check(ip2).is_some());
        assert!(limiter.check(ip2).is_none());
    }

    #[test]
    fn test_rate_limiter_returns_remaining() {
        let limiter = RateLimiterState::new(5);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3));

        assert_eq!(limiter.check(ip), Some(4));
        assert_eq!(limiter.check(ip), Some(3));
        assert_eq!(limiter.check(ip), Some(2));
        assert_eq!(limiter.check(ip), Some(1));
        assert_eq!(limiter.check(ip), Some(0));
        assert_eq!(limiter.check(ip), None);
    }
}
