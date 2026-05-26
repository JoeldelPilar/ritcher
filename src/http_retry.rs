//! HTTP fetch with automatic retry and backoff.
//!
//! Provides [`fetch_with_retry`] to deduplicate the retry pattern that was
//! previously copy-pasted in `handlers/ad.rs`, `handlers/segment.rs`, and
//! `ad/vast_provider.rs`.

use reqwest::{Client, Response};
use std::time::Duration;
use tracing::warn;

/// Default number of fetch attempts (1 initial + 1 retry).
pub const DEFAULT_MAX_ATTEMPTS: u32 = 2;

/// Default backoff between attempts in milliseconds.
pub const DEFAULT_BACKOFF_MS: u64 = 500;

/// Configuration for [`fetch_with_retry`].
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Total number of attempts (minimum 1; 0 is treated as 1).
    pub max_attempts: u32,
    /// Sleep duration between consecutive attempts.
    pub backoff: Duration,
    /// Optional per-request timeout applied to each individual attempt.
    ///
    /// When `None`, the client's own timeout applies.
    pub timeout: Option<Duration>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            backoff: Duration::from_millis(DEFAULT_BACKOFF_MS),
            timeout: None,
        }
    }
}

/// Retry semantics for a Stitcher HTTP fetch — max attempts, fixed backoff,
/// optional per-attempt timeout.
///
/// The policy borrows the [`Client`] it executes against rather than owning
/// one: there is exactly one shared `reqwest::Client` in the process (with
/// custom SSRF redirect policy and `SsrfSafeResolver`), and multiple call
/// sites compose different policies against that same client.
///
/// Construct via [`RetryPolicy::production`] or [`RetryPolicy::no_retries`]
/// and adjust with the `with_*` builder methods. There is intentionally no
/// `Default` impl — `production()` is named so the intent is greppable at
/// the call site.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    max_attempts: u32,
    backoff: Duration,
    timeout: Option<Duration>,
}

impl RetryPolicy {
    /// Production default: 2 attempts (1 initial + 1 retry), 500ms backoff,
    /// no per-attempt timeout override (the client's own timeout applies).
    pub fn production() -> Self {
        Self {
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            backoff: Duration::from_millis(DEFAULT_BACKOFF_MS),
            timeout: None,
        }
    }

    /// Single attempt, zero backoff. For tests and fail-fast paths where
    /// retrying would mask the error under investigation.
    pub fn no_retries() -> Self {
        Self {
            max_attempts: 1,
            backoff: Duration::from_millis(0),
            timeout: None,
        }
    }

    /// Override the per-attempt timeout. Composes onto an existing policy.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Override the backoff between attempts. Composes onto an existing policy.
    pub fn with_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
        self
    }

    /// Override the maximum attempt count. Composes onto an existing policy.
    /// Values below 1 are treated as 1 by [`RetryPolicy::execute`].
    pub fn with_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    /// Maximum number of attempts this policy will make.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Fixed sleep between consecutive attempts.
    pub fn backoff(&self) -> Duration {
        self.backoff
    }

    /// Optional per-attempt timeout. `None` means the client's own timeout
    /// applies unmodified.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// Execute an HTTP GET against `url` using `client`, applying this
    /// policy's retry semantics.
    ///
    /// Behavior: makes up to `max_attempts` GET requests against `url`,
    /// sleeping `backoff` between consecutive attempts. The backoff is
    /// **fixed** — not exponential, no jitter. Each attempt applies the
    /// optional per-attempt `timeout` on top of any client-level timeout.
    /// `max_attempts == 0` is silently clamped to 1.
    ///
    /// Returns the first successful (2xx) [`Response`], or the last
    /// [`reqwest::Error`] encountered once all attempts are exhausted. Note
    /// that intermediate non-2xx responses are logged and retried; only the
    /// final attempt's non-2xx status is converted to an error via
    /// `error_for_status()`.
    ///
    /// # Errors
    ///
    /// Returns the last network or non-2xx error after all retries fail.
    pub async fn execute(&self, client: &Client, url: &str) -> Result<Response, reqwest::Error> {
        // Step 1 (parallel-spar): delegate to the existing free function.
        // Step 6 will invert this so the loop body lives here.
        let config = RetryConfig {
            max_attempts: self.max_attempts,
            backoff: self.backoff,
            timeout: self.timeout,
        };
        fetch_with_retry(client, url, &config).await
    }
}

/// Fetch a URL via HTTP GET with automatic retry and backoff.
///
/// Attempts the request up to `config.max_attempts` times, sleeping
/// `config.backoff` between each attempt.
///
/// Returns the first successful (2xx) [`Response`], or the last
/// [`reqwest::Error`] encountered once all attempts are exhausted.
///
/// # Errors
///
/// Returns the last network or non-2xx error after all retries fail.
pub async fn fetch_with_retry(
    client: &Client,
    url: &str,
    config: &RetryConfig,
) -> Result<Response, reqwest::Error> {
    let max_attempts = config.max_attempts.max(1);

    // Retry loop: attempts 1 through N-1, with backoff between each.
    // The final attempt is handled separately below to guarantee a
    // return without `unreachable!()` or other panic paths.
    for attempt in 1..max_attempts {
        let mut request = client.get(url);
        if let Some(timeout) = config.timeout {
            request = request.timeout(timeout);
        }

        match request.send().await {
            Ok(response) if response.status().is_success() => return Ok(response),

            Ok(response) => {
                warn!(
                    "HTTP fetch returned {} for {} (attempt {}/{})",
                    response.status(),
                    url,
                    attempt,
                    max_attempts
                );
            }

            Err(e) => {
                warn!(
                    "HTTP fetch failed for {} (attempt {}/{}): {}",
                    url, attempt, max_attempts, e
                );
            }
        }

        warn!("Retrying HTTP fetch in {}ms...", config.backoff.as_millis());
        tokio::time::sleep(config.backoff).await;
    }

    // Final attempt — returns directly, no further retry
    let mut request = client.get(url);
    if let Some(timeout) = config.timeout {
        request = request.timeout(timeout);
    }

    let response = request.send().await.map_err(|e| {
        warn!(
            "HTTP fetch failed for {} (attempt {}/{}): {}",
            url, max_attempts, max_attempts, e
        );
        e
    })?;

    if !response.status().is_success() {
        warn!(
            "HTTP fetch returned {} for {} (attempt {}/{})",
            response.status(),
            url,
            max_attempts,
            max_attempts
        );
    }

    response.error_for_status()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn retry_config_defaults() {
        let cfg = RetryConfig::default();
        assert_eq!(cfg.max_attempts, DEFAULT_MAX_ATTEMPTS);
        assert_eq!(cfg.backoff, Duration::from_millis(DEFAULT_BACKOFF_MS));
        assert!(cfg.timeout.is_none());
    }

    #[test]
    fn retry_config_custom() {
        let cfg = RetryConfig {
            max_attempts: 5,
            backoff: Duration::from_millis(100),
            timeout: Some(Duration::from_secs(10)),
        };
        assert_eq!(cfg.max_attempts, 5);
        assert_eq!(cfg.backoff, Duration::from_millis(100));
        assert_eq!(cfg.timeout, Some(Duration::from_secs(10)));
    }

    #[test]
    fn max_attempts_zero_treated_as_one() {
        let cfg = RetryConfig {
            max_attempts: 0,
            ..Default::default()
        };
        // max(1) guard ensures at least one attempt
        assert_eq!(cfg.max_attempts.max(1), 1);
    }

    #[test]
    fn retry_config_is_debug() {
        let cfg = RetryConfig::default();
        let debug = format!("{:?}", cfg);
        assert!(debug.contains("max_attempts"));
        assert!(debug.contains("backoff"));
    }

    #[test]
    fn retry_config_is_clone() {
        let cfg = RetryConfig {
            max_attempts: 3,
            backoff: Duration::from_millis(200),
            timeout: Some(Duration::from_secs(5)),
        };
        let cloned = cfg.clone();
        assert_eq!(cloned.max_attempts, 3);
        assert_eq!(cloned.backoff, Duration::from_millis(200));
        assert_eq!(cloned.timeout, Some(Duration::from_secs(5)));
    }

    // ---- RetryPolicy constructor & builder unit tests (Step 1) ----

    #[test]
    fn production_constructor_knobs() {
        let policy = RetryPolicy::production();
        assert_eq!(policy.max_attempts(), DEFAULT_MAX_ATTEMPTS);
        assert_eq!(policy.backoff(), Duration::from_millis(DEFAULT_BACKOFF_MS));
        assert!(policy.timeout().is_none());
    }

    #[test]
    fn no_retries_constructor_knobs() {
        let policy = RetryPolicy::no_retries();
        assert_eq!(policy.max_attempts(), 1);
        assert_eq!(policy.backoff(), Duration::from_millis(0));
        assert!(policy.timeout().is_none());
    }

    #[test]
    fn with_attempts_overrides_only_max_attempts() {
        let policy = RetryPolicy::production().with_attempts(5);
        assert_eq!(policy.max_attempts(), 5);
        // Other knobs preserved from production()
        assert_eq!(policy.backoff(), Duration::from_millis(DEFAULT_BACKOFF_MS));
        assert!(policy.timeout().is_none());
    }

    #[test]
    fn with_timeout_composes_with_production() {
        // Mirrors the VAST call site: production() + with_timeout(self.timeout)
        let t = Duration::from_secs(7);
        let policy = RetryPolicy::production().with_timeout(t);
        assert_eq!(policy.timeout(), Some(t));
        assert_eq!(policy.max_attempts(), DEFAULT_MAX_ATTEMPTS);
        assert_eq!(policy.backoff(), Duration::from_millis(DEFAULT_BACKOFF_MS));
    }

    #[test]
    fn with_backoff_overrides_only_backoff() {
        let b = Duration::from_millis(42);
        let policy = RetryPolicy::production().with_backoff(b);
        assert_eq!(policy.backoff(), b);
        assert_eq!(policy.max_attempts(), DEFAULT_MAX_ATTEMPTS);
        assert!(policy.timeout().is_none());
    }

    #[test]
    fn retry_policy_is_debug() {
        let policy = RetryPolicy::production();
        let debug = format!("{:?}", policy);
        assert!(debug.contains("max_attempts"));
        assert!(debug.contains("backoff"));
    }

    #[test]
    fn retry_policy_is_clone() {
        let policy = RetryPolicy::production()
            .with_attempts(3)
            .with_backoff(Duration::from_millis(200))
            .with_timeout(Duration::from_secs(5));
        let cloned = policy.clone();
        assert_eq!(cloned.max_attempts(), 3);
        assert_eq!(cloned.backoff(), Duration::from_millis(200));
        assert_eq!(cloned.timeout(), Some(Duration::from_secs(5)));
    }

    // ---- Integration tests using wiremock ----

    #[tokio::test]
    async fn succeeds_on_first_attempt() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let client = Client::new();
        let config = RetryConfig {
            backoff: Duration::from_millis(1),
            ..Default::default()
        };

        let result = fetch_with_retry(&client, &server.uri(), &config).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn retries_on_server_error_then_succeeds() {
        let server = MockServer::start().await;

        // 200 fallback (lower priority — mounted first)
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("recovered"))
            .mount(&server)
            .await;

        // 500 on first hit (higher priority — mounted last, deactivates after 1)
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        let client = Client::new();
        let config = RetryConfig {
            max_attempts: 2,
            backoff: Duration::from_millis(1),
            timeout: None,
        };

        let result = fetch_with_retry(&client, &server.uri(), &config).await;
        assert!(result.is_ok(), "Expected success after retry");
        assert_eq!(result.unwrap().text().await.unwrap(), "recovered");
    }

    #[tokio::test]
    async fn returns_error_after_all_retries_exhausted() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let client = Client::new();
        let config = RetryConfig {
            max_attempts: 2,
            backoff: Duration::from_millis(1),
            timeout: None,
        };

        let result = fetch_with_retry(&client, &server.uri(), &config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn single_attempt_no_retry() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = Client::new();
        let config = RetryConfig {
            max_attempts: 1,
            backoff: Duration::from_millis(1),
            timeout: None,
        };

        let result = fetch_with_retry(&client, &server.uri(), &config).await;
        assert!(result.is_err());
    }
}
