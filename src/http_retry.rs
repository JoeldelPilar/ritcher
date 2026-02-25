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
