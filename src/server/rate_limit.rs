//! Per-IP rate limiting middleware.
//!
//! Fixed-window counter using DashMap. Protects origin and ad servers
//! from abusive traffic while allowing normal player request patterns.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;

use super::state::AppState;

/// Per-IP fixed-window rate limiter.
#[derive(Clone, Debug)]
pub struct RateLimiter {
    /// Counters per IP string: (request_count, window_start)
    counters: Arc<DashMap<String, (u32, Instant)>>,
    /// Max requests per window
    limit: u32,
    /// Window duration
    window: Duration,
}

impl RateLimiter {
    /// Create a new rate limiter with the given requests-per-minute limit.
    pub fn new(requests_per_minute: u32) -> Self {
        Self {
            counters: Arc::new(DashMap::new()),
            limit: requests_per_minute,
            window: Duration::from_secs(60),
        }
    }

    /// Check whether a request from `ip` is allowed.
    /// Returns `true` if under limit, `false` if rate-limited.
    fn check(&self, ip: &str) -> bool {
        let now = Instant::now();
        let mut entry = self.counters.entry(ip.to_string()).or_insert((0, now));

        // Reset window if expired
        if entry.1.elapsed() >= self.window {
            entry.0 = 0;
            entry.1 = now;
        }

        entry.0 += 1;
        entry.0 <= self.limit
    }

    /// Remove stale entries (windows that have expired). Call periodically.
    pub fn cleanup(&self) {
        self.counters
            .retain(|_, (_, window_start)| window_start.elapsed() < self.window);
    }
}

/// Extract client IP from X-Forwarded-For header or fall back to a default.
fn extract_client_ip(req: &Request) -> String {
    // Check X-Forwarded-For (first IP is the original client)
    if let Some(forwarded) = req.headers().get("x-forwarded-for")
        && let Ok(value) = forwarded.to_str()
        && let Some(first_ip) = value.split(',').next()
    {
        let ip = first_ip.trim();
        if !ip.is_empty() {
            return ip.to_string();
        }
    }

    // Fall back when not behind a reverse proxy (local dev, direct access)
    "unknown".to_string()
}

/// Axum middleware: reject requests exceeding the per-IP rate limit.
pub async fn rate_limit_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(ref limiter) = state.rate_limiter {
        let ip = extract_client_ip(&req);
        if !limiter.check(&ip) {
            warn!("Rate limit exceeded for IP: {}", ip);
            return (StatusCode::TOO_MANY_REQUESTS, "Rate limit exceeded\n").into_response();
        }
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_requests_under_limit() {
        let limiter = RateLimiter::new(5);
        for _ in 0..5 {
            assert!(limiter.check("192.168.1.1"));
        }
    }

    #[test]
    fn blocks_requests_over_limit() {
        let limiter = RateLimiter::new(3);
        assert!(limiter.check("10.0.0.1"));
        assert!(limiter.check("10.0.0.1"));
        assert!(limiter.check("10.0.0.1"));
        assert!(!limiter.check("10.0.0.1"), "4th request should be blocked");
    }

    #[test]
    fn different_ips_have_separate_limits() {
        let limiter = RateLimiter::new(2);
        assert!(limiter.check("10.0.0.1"));
        assert!(limiter.check("10.0.0.1"));
        assert!(!limiter.check("10.0.0.1"));

        // Different IP should still be allowed
        assert!(limiter.check("10.0.0.2"));
        assert!(limiter.check("10.0.0.2"));
    }

    #[test]
    fn window_resets_after_expiry() {
        let limiter = RateLimiter {
            counters: Arc::new(DashMap::new()),
            limit: 2,
            window: Duration::from_millis(1),
        };

        assert!(limiter.check("10.0.0.1"));
        assert!(limiter.check("10.0.0.1"));
        assert!(!limiter.check("10.0.0.1"));

        // Wait for window to expire
        std::thread::sleep(Duration::from_millis(5));

        assert!(
            limiter.check("10.0.0.1"),
            "Should be allowed after window reset"
        );
    }

    #[test]
    fn cleanup_removes_stale_entries() {
        let limiter = RateLimiter {
            counters: Arc::new(DashMap::new()),
            limit: 10,
            window: Duration::from_millis(1),
        };

        limiter.check("10.0.0.1");
        limiter.check("10.0.0.2");
        assert_eq!(limiter.counters.len(), 2);

        std::thread::sleep(Duration::from_millis(5));
        limiter.cleanup();

        assert_eq!(limiter.counters.len(), 0, "Stale entries should be removed");
    }
}
