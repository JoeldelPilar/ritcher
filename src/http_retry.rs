//! HTTP fetch with automatic retry and backoff.
//!
//! Provides [`RetryPolicy`] as a first-class domain type for HTTP retry
//! semantics. The policy borrows a shared [`Client`] (one per process) and
//! exposes an [`execute`](RetryPolicy::execute) method that performs the
//! retry loop with fixed backoff and an optional per-attempt timeout.

use reqwest::{Client, Response};
use std::time::Duration;
use tracing::warn;

/// Default number of fetch attempts (1 initial + 1 retry). Used by
/// [`RetryPolicy::production`]. Private — adjust via builder methods.
const DEFAULT_MAX_ATTEMPTS: u32 = 2;

/// Default backoff between attempts in milliseconds. Used by
/// [`RetryPolicy::production`]. Private — adjust via builder methods.
const DEFAULT_BACKOFF_MS: u64 = 500;

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
        let max_attempts = self.max_attempts.max(1);

        // Retry loop: attempts 1 through N-1, with backoff between each.
        // The final attempt is handled separately below to guarantee a
        // return without `unreachable!()` or other panic paths.
        for attempt in 1..max_attempts {
            let mut request = client.get(url);
            if let Some(timeout) = self.timeout {
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

            warn!("Retrying HTTP fetch in {}ms...", self.backoff.as_millis());
            tokio::time::sleep(self.backoff).await;
        }

        // Final attempt — returns directly, no further retry
        let mut request = client.get(url);
        if let Some(timeout) = self.timeout {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ---- RetryPolicy constructor & builder unit tests ----

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

    // ---- Integration tests using wiremock (RetryPolicy::execute) ----

    #[tokio::test]
    async fn succeeds_on_first_attempt() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let client = Client::new();
        let policy = RetryPolicy::production().with_backoff(Duration::from_millis(1));

        let result = policy.execute(&client, &server.uri()).await;
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
        let policy = RetryPolicy::production()
            .with_attempts(2)
            .with_backoff(Duration::from_millis(1));

        let result = policy.execute(&client, &server.uri()).await;
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
        let policy = RetryPolicy::production()
            .with_attempts(2)
            .with_backoff(Duration::from_millis(1));

        let result = policy.execute(&client, &server.uri()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn no_retries_makes_exactly_one_request() {
        // Plan §5: the deepening pass argued for this test — the constructor
        // name documents the invariant. Asserts on request count, not just
        // Result, so a silent regression in attempt-count semantics fails here.
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let client = Client::new();
        let policy = RetryPolicy::no_retries();

        let result = policy.execute(&client, &server.uri()).await;
        assert!(result.is_err());
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "no_retries() must make exactly one request"
        );
    }

    #[tokio::test]
    async fn execute_treats_zero_attempts_as_one() {
        // max_attempts == 0 is silently clamped to 1 by execute().
        // Test that against a 200 endpoint: a single successful request is
        // made and the response is returned (clamp is to 1, not 0).
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("clamped"))
            .mount(&server)
            .await;

        let client = Client::new();
        let policy = RetryPolicy::production()
            .with_attempts(0)
            .with_backoff(Duration::from_millis(1));

        let result = policy.execute(&client, &server.uri()).await;
        assert!(result.is_ok(), "zero attempts should clamp to one");
        assert_eq!(result.unwrap().text().await.unwrap(), "clamped");
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "clamp must produce exactly one request"
        );
    }

    #[tokio::test]
    async fn execute_uses_provided_client() {
        // Proves the policy borrows the caller's client rather than
        // constructing one: a client with a default header should send
        // that header on every request issued by execute().
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(wiremock::matchers::header("X-Marker", "from-caller"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("X-Marker", "from-caller".parse().unwrap());
        let client = Client::builder().default_headers(headers).build().unwrap();

        let policy = RetryPolicy::no_retries();
        let result = policy.execute(&client, &server.uri()).await;
        assert!(
            result.is_ok(),
            "Mock only matches when the marker header is present, so success proves the policy used the provided client"
        );
    }
}
